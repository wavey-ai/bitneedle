// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Canonical BRD1 descriptor construction and serialization.

use anyhow::{bail, Context, Result};

use record_core::SpiralFamily;
use record_descriptor::{
    compute_descriptor_crc32, encode_cache_encryption_descriptor, encode_toned_carrier_map,
    payload_encoding_code, record_profile_code, release_id_to_bytes, CacheEncryptionDescriptor,
    SignedReleaseReference, ToneSpanDescriptor, PAYLOAD_ENCODING_RGB, PAYLOAD_ENCODING_TONED_V1,
    RECORD_DESCRIPTOR_MAGIC, RECORD_DESCRIPTOR_PREFIX_LENGTH, RECORD_DESCRIPTOR_VERSION,
    RECORD_DESCRIPTOR_VERSION_V3,
    encode_isrc_segment, normalize_upc, TrackIsrc,
    SEGMENT_ADDITIONAL_SIGNATURES, SEGMENT_CHAIN_ANCHOR,
    SEGMENT_DEFERRED_ATTESTATION, SEGMENT_ISRC, SEGMENT_UPC,
    SEGMENT_ARTIST, SEGMENT_ARTWORK_CREDIT, SEGMENT_BSC_POINTER, SEGMENT_CACHE_ENCRYPTION,
    SEGMENT_CANONICAL_URL, SEGMENT_CATALOG_NUMBER, SEGMENT_COPYRIGHT_HOLDER,
    SEGMENT_COPYRIGHT_YEAR, SEGMENT_CREATED_AT, SEGMENT_DESCRIPTOR_CRC32, SEGMENT_LABEL,
    SEGMENT_PAYLOAD_ENCODING, SEGMENT_RECORD_PROFILE, SEGMENT_RELEASE_ID,
    SEGMENT_SIGNED_RELEASE_REFERENCE, SEGMENT_SPIRAL_GEOMETRY, SEGMENT_STREAM_BYTE_LENGTH,
    SEGMENT_TITLE, SEGMENT_TONED_CARRIER_MAP,
};

pub const RECORD_DESCRIPTOR_TEXT_LIMIT: usize = 96;
pub const RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT: usize = 1024;

#[derive(Debug, Clone, Default)]
pub struct RecordDescriptorInput {
    pub record_profile: String,
    pub stream_byte_length: usize,
    pub payload_encoding: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub release_id: Option<String>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub artwork_credit: Option<String>,
    pub canonical_url: Option<String>,
    pub created_at: Option<u64>,
    pub copyright_year: Option<u16>,
    pub copyright_holder: Option<String>,
    pub signed_release_reference: Option<SignedReleaseReference>,
    pub bsc_pointer: Option<Vec<u8>>,
    pub tone_spans: Vec<ToneSpanDescriptor>,
    pub cache_encryption: Option<CacheEncryptionDescriptor>,
    /// The deferred group: written after the press, and never unsigned.
    pub chain_anchor: Option<Vec<u8>>,
    pub isrcs: Vec<TrackIsrc>,
    pub upc: Option<String>,
    pub deferred_attestation: Option<SignedReleaseReference>,
    /// Signatures beyond the first: a pressing may be attested by the
    /// artist, by yl.vin, or by both.
    pub additional_signatures: Vec<SignedReleaseReference>,
    /// The groove geometry family. Archimedean writes a v2 descriptor,
    /// byte-identical to every record before spiral families existed;
    /// vari-pitch writes the house v3 descriptor with a spiral-geometry
    /// segment.
    pub spiral_family: SpiralFamily,
}

pub fn encode_signed_release_reference(reference: &SignedReleaseReference) -> Result<Vec<u8>> {
    reference.validate()?;

    let key_id_len = u16::try_from(reference.key_id.len()).context("key ID exceeds u16")?;

    let mut out = Vec::new();
    out.push(reference.version);
    out.extend_from_slice(&reference.release_commitment_sha256);
    out.extend_from_slice(&key_id_len.to_be_bytes());
    out.extend_from_slice(&reference.key_id);
    out.extend_from_slice(&reference.signature);
    Ok(out)
}

pub fn encode_additional_signatures(
    references: &[SignedReleaseReference],
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(
        &u16::try_from(references.len())
            .context("signature count exceeds u16")?
            .to_be_bytes(),
    );
    for reference in references {
        let encoded = encode_signed_release_reference(reference)?;
        out.extend_from_slice(
            &u16::try_from(encoded.len())
                .context("signature exceeds u16")?
                .to_be_bytes(),
        );
        out.extend_from_slice(&encoded);
    }
    Ok(out)
}

pub fn encode_record_descriptor_stream(
    b_value: f64,
    descriptor: &RecordDescriptorInput,
    byte_capacity: usize,
) -> Result<Vec<u8>> {
    if !(b_value.is_finite() && b_value > 0.0) {
        bail!("a positive finite b_value is required");
    }

    let (body, segment_count) = encode_segmented_body(descriptor)?;
    let payload_len = RECORD_DESCRIPTOR_PREFIX_LENGTH
        .checked_add(body.len())
        .context("record descriptor length overflow")?;

    if payload_len > byte_capacity {
        bail!("record descriptor exceeds combined lead-in and lead-out capacity");
    }
    if payload_len > u16::MAX as usize {
        bail!("record descriptor payload is too large");
    }

    let version = if descriptor.spiral_family.is_archimedean() {
        RECORD_DESCRIPTOR_VERSION
    } else {
        RECORD_DESCRIPTOR_VERSION_V3
    };

    let mut full = Vec::with_capacity(payload_len);
    full.extend_from_slice(RECORD_DESCRIPTOR_MAGIC);
    full.push(version);
    full.extend_from_slice(&(payload_len as u16).to_be_bytes());
    full.extend_from_slice(&segment_count.to_be_bytes());
    full.extend_from_slice(&(body.len() as u16).to_be_bytes());
    full.extend_from_slice(&b_value.to_bits().to_be_bytes());
    full.extend_from_slice(&body);

    let crc32 = compute_descriptor_crc32(&full);
    full[RECORD_DESCRIPTOR_PREFIX_LENGTH + 3..RECORD_DESCRIPTOR_PREFIX_LENGTH + 7]
        .copy_from_slice(&crc32.to_be_bytes());

    Ok(full)
}

pub fn encode_segmented_body(descriptor: &RecordDescriptorInput) -> Result<(Vec<u8>, u16)> {
    if descriptor.stream_byte_length == 0 {
        bail!("stream byte length must not be zero");
    }
    let stream_byte_length =
        u32::try_from(descriptor.stream_byte_length).context("stream byte length exceeds u32")?;

    let record_profile = vec![record_profile_code(&descriptor.record_profile)?];
    let payload_encoding_text = descriptor
        .payload_encoding
        .as_deref()
        .unwrap_or(PAYLOAD_ENCODING_RGB);
    let payload_encoding = vec![payload_encoding_code(payload_encoding_text)?];

    let title = optional_text(
        descriptor.title.as_deref(),
        RECORD_DESCRIPTOR_TEXT_LIMIT,
        "title",
    )?;
    let artist = optional_text(
        descriptor.artist.as_deref(),
        RECORD_DESCRIPTOR_TEXT_LIMIT,
        "artist",
    )?;
    let release_id = descriptor
        .release_id
        .as_deref()
        .map(release_id_to_bytes)
        .transpose()
        .context("release ID")?
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    let catalog_number = optional_text(
        descriptor.catalog_number.as_deref(),
        RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
        "catalog number",
    )?;
    let label = optional_text(
        descriptor.label.as_deref(),
        RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
        "label",
    )?;
    let artwork_credit = optional_text(
        descriptor.artwork_credit.as_deref(),
        RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
        "artwork credit",
    )?;
    let canonical_url = optional_text(
        descriptor.canonical_url.as_deref(),
        RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
        "canonical URL",
    )?;
    let created_at = descriptor
        .created_at
        .map(|millis| millis.to_be_bytes().to_vec())
        .unwrap_or_default();
    let copyright_year = descriptor
        .copyright_year
        .map(|year| year.to_be_bytes().to_vec())
        .unwrap_or_default();
    let copyright_holder = optional_text(
        descriptor.copyright_holder.as_deref(),
        RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
        "copyright holder",
    )?;
    let signed_release_reference = descriptor
        .signed_release_reference
        .as_ref()
        .map(encode_signed_release_reference)
        .transpose()?
        .unwrap_or_default();
    let bsc_pointer = descriptor.bsc_pointer.clone().unwrap_or_default();
    let chain_anchor = descriptor.chain_anchor.clone().unwrap_or_default();
    let isrcs = if descriptor.isrcs.is_empty() {
        Vec::new()
    } else {
        encode_isrc_segment(&descriptor.isrcs)?
    };
    let upc = descriptor
        .upc
        .as_deref()
        .map(normalize_upc)
        .transpose()?
        .map(|value| value.into_bytes())
        .unwrap_or_default();
    let deferred_attestation = descriptor
        .deferred_attestation
        .as_ref()
        .map(encode_signed_release_reference)
        .transpose()?
        .unwrap_or_default();
    let additional_signatures = if descriptor.additional_signatures.is_empty() {
        Vec::new()
    } else {
        if descriptor.signed_release_reference.is_none() {
            bail!("additional signatures without a signed release reference to join");
        }
        encode_additional_signatures(&descriptor.additional_signatures)?
    };
    // Null or signed, refused at the point of writing as well as reading.
    if (!chain_anchor.is_empty() || !isrcs.is_empty() || !upc.is_empty())
        == deferred_attestation.is_empty()
    {
        bail!(
            "the deferred group is null or signed: a chain anchor, ISRC or barcode \
             requires its attestation, and the attestation requires something to sign"
        );
    }
    let cache_encryption = descriptor
        .cache_encryption
        .as_ref()
        .map(encode_cache_encryption_descriptor)
        .transpose()?
        .unwrap_or_default();

    let spiral_geometry = match descriptor.spiral_family {
        SpiralFamily::Archimedean => Vec::new(),
        SpiralFamily::VariPitch {
            depth,
            seed,
            definition,
            sheen,
            placement,
            fire,
            tuning,
        } => {
            descriptor.spiral_family.validate()?;
            let mut payload = Vec::with_capacity(90);
            payload.push(descriptor.spiral_family.wire_code());
            payload.extend_from_slice(&depth.to_bits().to_be_bytes());
            payload.extend_from_slice(&seed.to_be_bytes());
            payload.extend_from_slice(&definition.to_bits().to_be_bytes());
            payload.extend_from_slice(&sheen.to_bits().to_be_bytes());
            payload.push(placement.wire_code());
            payload.extend_from_slice(&fire.to_bits().to_be_bytes());
            for value in [
                tuning.wave_one_cycles,
                tuning.wave_two_cycles,
                tuning.wave_balance,
                tuning.dither_frequency,
                tuning.aura_width,
                tuning.fire_cycles,
            ] {
                payload.extend_from_slice(&value.to_bits().to_be_bytes());
            }
            payload
        }
    };

    let toned_carrier_map = match payload_encoding_text {
        PAYLOAD_ENCODING_RGB => {
            if !descriptor.tone_spans.is_empty() {
                bail!("rgb payload encoding must not include tone spans");
            }
            Vec::new()
        }
        PAYLOAD_ENCODING_TONED_V1 => {
            if descriptor.tone_spans.is_empty() {
                bail!("toned-v1 payload encoding requires tone spans");
            }
            encode_toned_carrier_map(&descriptor.tone_spans, Some(descriptor.stream_byte_length))?
        }
        other => bail!("unsupported canonical payload encoding {other}"),
    };

    let mut out = Vec::new();
    let mut segment_count = 0u16;

    for (kind, payload) in [
        (SEGMENT_DESCRIPTOR_CRC32, 0u32.to_be_bytes().to_vec()),
        (
            SEGMENT_STREAM_BYTE_LENGTH,
            stream_byte_length.to_be_bytes().to_vec(),
        ),
        (SEGMENT_RECORD_PROFILE, record_profile),
        (SEGMENT_PAYLOAD_ENCODING, payload_encoding),
        (SEGMENT_TITLE, title),
        (SEGMENT_ARTIST, artist),
        (SEGMENT_RELEASE_ID, release_id),
        (SEGMENT_CATALOG_NUMBER, catalog_number),
        (SEGMENT_LABEL, label),
        (SEGMENT_ARTWORK_CREDIT, artwork_credit),
        (SEGMENT_CANONICAL_URL, canonical_url),
        (SEGMENT_CREATED_AT, created_at),
        (SEGMENT_COPYRIGHT_YEAR, copyright_year),
        (SEGMENT_COPYRIGHT_HOLDER, copyright_holder),
        (SEGMENT_SIGNED_RELEASE_REFERENCE, signed_release_reference),
        (SEGMENT_BSC_POINTER, bsc_pointer),
        (SEGMENT_TONED_CARRIER_MAP, toned_carrier_map),
        (SEGMENT_CACHE_ENCRYPTION, cache_encryption),
        (SEGMENT_CHAIN_ANCHOR, chain_anchor),
        (SEGMENT_ISRC, isrcs),
        (SEGMENT_UPC, upc),
        (SEGMENT_DEFERRED_ATTESTATION, deferred_attestation),
        (SEGMENT_ADDITIONAL_SIGNATURES, additional_signatures),
        (SEGMENT_SPIRAL_GEOMETRY, spiral_geometry),
    ] {
        if payload.is_empty() {
            continue;
        }
        push_segment(&mut out, kind, &payload)?;
        segment_count = segment_count
            .checked_add(1)
            .context("record descriptor segment count overflow")?;
    }

    Ok((out, segment_count))
}

pub fn push_segment(out: &mut Vec<u8>, kind: u8, payload: &[u8]) -> Result<()> {
    if payload.len() > u16::MAX as usize {
        bail!("record descriptor segment {kind} exceeds length limit");
    }
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

pub fn optional_text(value: Option<&str>, limit: usize, label: &str) -> Result<Vec<u8>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let normalized = value.trim();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    if normalized.chars().any(char::is_control) {
        bail!("{label} must not contain control characters");
    }
    if normalized.len() > limit {
        bail!("{label} exceeds record descriptor text limit");
    }
    Ok(normalized.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use record_descriptor::{
        CacheEncryptionAlgorithm, CacheKeyDerivation, ToneOrdering,
        CACHE_ENCRYPTION_DESCRIPTOR_VERSION, CACHE_ENCRYPTION_SECRET_LENGTH,
    };

    fn base_input() -> RecordDescriptorInput {
        RecordDescriptorInput {
            record_profile: "single45".to_string(),
            stream_byte_length: 12,
            payload_encoding: Some(PAYLOAD_ENCODING_RGB.to_string()),
            ..Default::default()
        }
    }

    fn cache_encryption_descriptor() -> CacheEncryptionDescriptor {
        CacheEncryptionDescriptor {
            version: CACHE_ENCRYPTION_DESCRIPTOR_VERSION,
            algorithm: CacheEncryptionAlgorithm::XChaCha20Poly1305,
            key_derivation: CacheKeyDerivation::HkdfSha256,
            secret: vec![7u8; CACHE_ENCRYPTION_SECRET_LENGTH],
        }
    }

    #[test]
    fn rgb_rejects_tone_spans() {
        let mut input = base_input();
        input.tone_spans.push(ToneSpanDescriptor {
            byte_length: 12,
            base: [255, 192, 203],
            luma_tolerance: 16,
            bits_per_pixel: 21,
            ordering: ToneOrdering::ChromaProximity,
        });

        assert!(encode_record_descriptor_stream(1.0, &input, 4096).is_err());
    }

    #[test]
    fn toned_v1_requires_and_encodes_map() {
        let mut input = base_input();
        input.payload_encoding = Some(PAYLOAD_ENCODING_TONED_V1.to_string());
        input.tone_spans.push(ToneSpanDescriptor {
            byte_length: 12,
            base: [255, 192, 203],
            luma_tolerance: 16,
            bits_per_pixel: 21,
            ordering: ToneOrdering::ChromaProximity,
        });

        let bytes = encode_record_descriptor_stream(1.0, &input, 4096).unwrap();
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).unwrap();

        assert_eq!(decoded.payload_encoding, PAYLOAD_ENCODING_TONED_V1);
        assert_eq!(decoded.tone_spans, input.tone_spans);
    }

    #[test]
    fn toned_v1_rejects_incomplete_coverage() {
        let mut input = base_input();
        input.payload_encoding = Some(PAYLOAD_ENCODING_TONED_V1.to_string());
        input.tone_spans.push(ToneSpanDescriptor {
            byte_length: 11,
            base: [255, 192, 203],
            luma_tolerance: 16,
            bits_per_pixel: 21,
            ordering: ToneOrdering::ChromaProximity,
        });

        assert!(encode_record_descriptor_stream(1.0, &input, 4096).is_err());
    }

    #[test]
    fn vari_pitch_descriptor_round_trips_as_v3() {
        let mut input = base_input();
        input.spiral_family = SpiralFamily::VariPitch {
            depth: 0.28,
            seed: 0xDEC0_DE00_5EED_0001,
            definition: 0.65,
            sheen: 0.8,
            placement: record_core::VariPitchPlacement::Inner,
            fire: 0.0,
            tuning: record_core::VariPitchTuning::default(),
        };

        let bytes = encode_record_descriptor_stream(1.0, &input, 4096).unwrap();
        assert_eq!(bytes[4], RECORD_DESCRIPTOR_VERSION_V3);

        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).unwrap();
        assert_eq!(decoded.version, RECORD_DESCRIPTOR_VERSION_V3);
        assert_eq!(decoded.spiral_family, input.spiral_family);
    }

    #[test]
    fn archimedean_descriptor_stays_v2_with_no_spiral_segment() {
        let bytes = encode_record_descriptor_stream(1.0, &base_input(), 4096).unwrap();
        assert_eq!(bytes[4], RECORD_DESCRIPTOR_VERSION);
        assert!(
            !bytes.contains(&SEGMENT_SPIRAL_GEOMETRY)
                || record_descriptor::decode_record_descriptor_bytes(&bytes)
                    .unwrap()
                    .spiral_family
                    == SpiralFamily::Archimedean,
            "archimedean descriptors must not grow a spiral geometry segment"
        );
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).unwrap();
        assert_eq!(decoded.spiral_family, SpiralFamily::Archimedean);
    }

    #[test]
    fn vari_pitch_depth_out_of_range_is_refused() {
        let mut input = base_input();
        input.spiral_family = SpiralFamily::VariPitch {
            depth: 0.6,
            seed: 1,
            definition: 0.0,
            sheen: 0.0,
            placement: record_core::VariPitchPlacement::Even,
            fire: 0.0,
            tuning: record_core::VariPitchTuning::default(),
        };
        assert!(encode_record_descriptor_stream(1.0, &input, 4096).is_err());
    }

    #[test]
    fn cache_encryption_round_trips() {
        let mut input = base_input();
        input.cache_encryption = Some(cache_encryption_descriptor());

        let bytes = encode_record_descriptor_stream(1.0, &input, 4096).unwrap();
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).unwrap();

        assert_eq!(
            decoded.cache_encryption,
            Some(cache_encryption_descriptor())
        );
    }
}
