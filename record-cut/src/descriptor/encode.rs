// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Canonical BRD1 descriptor construction and serialization.

use anyhow::{bail, Context, Result};

use record_core::SpiralFamily;
use record_descriptor::{
    compute_descriptor_crc32, encode_cache_encryption_descriptor, encode_isrc_segment,
    encode_toned_carrier_map, normalize_upc, payload_encoding_code, record_profile_code,
    release_id_to_bytes, CacheEncryptionDescriptor, DeadwaxExtent, SignedReleaseReference,
    ToneSpanDescriptor,
    TrackIsrc, PAYLOAD_ENCODING_RGB, PAYLOAD_ENCODING_TONED_V1, RECORD_DESCRIPTOR_MAGIC,
    RECORD_DESCRIPTOR_PREFIX_LENGTH, RECORD_DESCRIPTOR_VERSION, RECORD_DESCRIPTOR_VERSION_HOUSE,
    SEGMENT_ADDITIONAL_SIGNATURES, SEGMENT_ARTIST, SEGMENT_ARTWORK_CREDIT, SEGMENT_BSC_POINTER,
    SEGMENT_CACHE_ENCRYPTION, SEGMENT_CANONICAL_URL, SEGMENT_CATALOG_NUMBER, SEGMENT_CHAIN_ANCHOR,
    SEGMENT_COPYRIGHT_HOLDER, SEGMENT_COPYRIGHT_YEAR, SEGMENT_CREATED_AT,
    SEGMENT_DEFERRED_ATTESTATION, SEGMENT_DESCRIPTOR_CRC32, SEGMENT_ISRC, SEGMENT_LABEL,
    SEGMENT_PAYLOAD_ENCODING, SEGMENT_RECORD_PROFILE, SEGMENT_RELEASE_ID,
    SEGMENT_SIGNED_RELEASE_REFERENCE, SEGMENT_SPIRAL_GEOMETRY, SEGMENT_STREAM_BYTE_LENGTH,
    SEGMENT_DEADWAX_EXTENT, SEGMENT_TITLE, SEGMENT_TONED_CARRIER_MAP, SEGMENT_UPC,
};

pub const RECORD_DESCRIPTOR_TEXT_LIMIT: usize = 96;
pub const RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT: usize = 1024;

#[derive(Debug, Clone, Default)]
pub struct RecordDescriptorInput {
    /// The radius, in rendered pixels, at which the programme's groove stops
    /// and the deadwax takes over. Zero for a cut that reaches the label.
    pub cut_inner_radius: u16,
    /// The deadwax's spiral `b`. The feed, never the turn count: a lathe's
    /// spiral lever does not know how far it has to travel, and neither does
    /// a reader — both derive the turns from the space that is left.
    pub deadwax_b_value: f64,
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
    /// The clockface for a toned-v2 groove; `None` for every other encoding.
    pub tone_clock: Option<record_descriptor::ToneClockDescriptor>,
    pub cache_encryption: Option<CacheEncryptionDescriptor>,
    /// The deferred group: written after the press, and never unsigned.
    pub chain_anchor: Option<Vec<u8>>,
    pub isrcs: Vec<TrackIsrc>,
    pub upc: Option<String>,
    pub deferred_attestation: Option<SignedReleaseReference>,
    /// Signatures beyond the first: a pressing may be attested by the
    /// artist, by yl.vin, or by both.
    pub additional_signatures: Vec<SignedReleaseReference>,
    /// The deadwax that the cut left, and the owner of any claim on it.
    /// `None` when the programme ran to the label and left no band. The
    /// renderer sets this field, because the extent states the radius at which
    /// the groove stopped.
    pub deadwax: Option<DeadwaxExtent>,
    /// The one tone the trailer is cut in, when the cut was given one.
    ///
    /// `None` is a trailer that follows the record's wheel, which a reader
    /// has out of the tone clock map, or an untoned record.
    pub run_out_tone: Option<[u8; 3]>,
    /// The groove geometry family. Archimedean writes a v2 descriptor,
    /// byte-identical to every record before spiral families existed;
    /// vari-pitch writes the house v3 descriptor with a spiral-geometry
    /// segment.
    pub spiral_family: SpiralFamily,
    /// Whether the programme's groove winds *anti*-clockwise from its start
    /// angle — the hand a lathe cuts, since the platter turns clockwise
    /// under a head that does not travel.
    ///
    /// Named for the departure rather than the state, because this struct
    /// derives `Default` and `bool::default()` is `false`: the default has
    /// to be the hand every record already carries, or a caller that fills
    /// this struct field by field silently cuts the other way.
    pub spiral_anticlockwise: bool,
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

pub fn encode_additional_signatures(references: &[SignedReleaseReference]) -> Result<Vec<u8>> {
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

    // A cut that reached the label declares no deadwax, and its feed is
    // meaningless rather than zero — write it as such instead of letting an
    // unset field read as an infinitely fine groove.
    let deadwax_b_value = if descriptor.cut_inner_radius == 0 {
        0.0
    } else {
        if !(descriptor.deadwax_b_value.is_finite() && descriptor.deadwax_b_value > 0.0) {
            bail!("a cut that stops short of the label must declare a positive deadwax feed");
        }
        descriptor.deadwax_b_value
    };

    let (body, segment_count) = encode_segmented_body(descriptor)?;
    let payload_len = RECORD_DESCRIPTOR_PREFIX_LENGTH
        .checked_add(body.len())
        .context("record descriptor length overflow")?;

    if payload_len > byte_capacity {
        bail!("record descriptor exceeds combined lead-in and trailer capacity");
    }
    if payload_len > u16::MAX as usize {
        bail!("record descriptor payload is too large");
    }

    let version = if descriptor.spiral_family.is_archimedean() {
        RECORD_DESCRIPTOR_VERSION
    } else {
        RECORD_DESCRIPTOR_VERSION_HOUSE
    };

    let mut full = Vec::with_capacity(payload_len);
    full.extend_from_slice(RECORD_DESCRIPTOR_MAGIC);
    full.push(version);
    full.extend_from_slice(&(payload_len as u16).to_be_bytes());
    full.extend_from_slice(&segment_count.to_be_bytes());
    full.extend_from_slice(&(body.len() as u16).to_be_bytes());
    full.extend_from_slice(&b_value.to_bits().to_be_bytes());
    full.extend_from_slice(&descriptor.cut_inner_radius.to_be_bytes());
    full.extend_from_slice(&deadwax_b_value.to_bits().to_be_bytes());
    full.extend_from_slice(&body);

    let crc32 = compute_descriptor_crc32(&full);
    full[RECORD_DESCRIPTOR_PREFIX_LENGTH + 3..RECORD_DESCRIPTOR_PREFIX_LENGTH + 7]
        .copy_from_slice(&crc32.to_be_bytes());

    Ok(full)
}

pub fn encode_segmented_body(descriptor: &RecordDescriptorInput) -> Result<(Vec<u8>, u16)> {
    // Empty for a clockwise cut, so an ordinary record's bytes and segment
    // count are exactly what they always were.
    let handedness: Vec<u8> = if descriptor.spiral_anticlockwise {
        vec![0u8]
    } else {
        Vec::new()
    };
    // Written on every record. The run-out is derived rather than declared,
    // so this byte is the only thing standing between a future change to the
    // gap ladder and a decoder that traces the wrong band through an old
    // record and hands back plausible rubbish.
    // Written on every record. The run-out is derived rather than declared,
    // so this byte is what stops a decoder tracing an old record's band with
    // new constants. The tone follows it where the trailer was cut in one:
    // the band is a carrier, and its palette is on no other segment.
    let mut lead_out_geometry: Vec<u8> = vec![record_descriptor::LEAD_OUT_GEOMETRY_REVISION];
    if let Some(tone) = descriptor.run_out_tone {
        lead_out_geometry.extend_from_slice(&tone);
    }

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
            if descriptor.tone_clock.is_some() {
                bail!("rgb payload encoding must not include a tone clock");
            }
            Vec::new()
        }
        PAYLOAD_ENCODING_TONED_V1 => {
            if descriptor.tone_spans.is_empty() {
                bail!("toned-v1 payload encoding requires tone spans");
            }
            if descriptor.tone_clock.is_some() {
                bail!("toned-v1 payload encoding must not include a tone clock");
            }
            encode_toned_carrier_map(&descriptor.tone_spans, Some(descriptor.stream_byte_length))?
        }
        record_descriptor::PAYLOAD_ENCODING_TONED_V2 => {
            if !descriptor.tone_spans.is_empty() {
                bail!("toned-v2 payload encoding must not include tone spans");
            }
            Vec::new()
        }
        other => bail!("unsupported canonical payload encoding {other}"),
    };
    let tone_clock_map = match (payload_encoding_text, descriptor.tone_clock.as_ref()) {
        (record_descriptor::PAYLOAD_ENCODING_TONED_V2, Some(clock)) => {
            record_descriptor::encode_tone_clock_map(clock, Some(descriptor.stream_byte_length))?
        }
        (record_descriptor::PAYLOAD_ENCODING_TONED_V2, None) => {
            bail!("toned-v2 payload encoding requires a tone clock")
        }
        _ => Vec::new(),
    };

    let deadwax = match descriptor.deadwax.as_ref() {
        Some(extent) => record_descriptor::encode_deadwax_extent(extent)?,
        None => Vec::new(),
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
        // Both of these say how the trailer is toned, and the stream crosses
        // into the trailer once the lead-in is full. A reader has to have
        // them before it gets there, so they go before any field a writer
        // chooses the length of.
        (
            record_descriptor::SEGMENT_LEAD_OUT_GEOMETRY,
            lead_out_geometry,
        ),
        (record_descriptor::SEGMENT_TONE_CLOCK_MAP, tone_clock_map),
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
        (SEGMENT_DEADWAX_EXTENT, deadwax),
        (record_descriptor::SEGMENT_GROOVE_HANDEDNESS, handedness),
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

    fn deadwax(claim: Option<[u8; 4]>, used: u32) -> DeadwaxExtent {
        DeadwaxExtent {
            outer_radius: 700,
            inner_radius: 420,
            pixel_capacity: 96_000,
            encoding: record_descriptor::DEADWAX_ENCODING_GRAYSCALE_NIBBLE,
            byte_capacity: 48_000,
            claim,
            claimed_byte_length: used,
        }
    }

    /// The band survives the stream: a reader that holds nothing but the
    /// descriptor learns where the deadwax is and that nobody is in it.
    #[test]
    fn a_free_deadwax_round_trips_through_the_stream() {
        let mut input = base_input();
        input.deadwax = Some(deadwax(None, 0));

        let bytes = encode_record_descriptor_stream(1.0, &input, 4096).expect("stream");
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).expect("decode");
        let extent = decoded.deadwax.expect("deadwax segment");

        assert!(extent.is_free());
        assert_eq!(extent.outer_radius, 700);
        assert_eq!(extent.inner_radius, 420);
        assert_eq!(extent.free_byte_capacity(), 48_000);
    }

    /// And a claimed one says who has it and how much of it is left.
    #[test]
    fn a_claimed_deadwax_carries_its_owner_and_what_is_left() {
        let mut input = base_input();
        input.deadwax = Some(deadwax(Some(*b"SIDE"), 12_000));

        let bytes = encode_record_descriptor_stream(1.0, &input, 4096).expect("stream");
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).expect("decode");
        let extent = decoded.deadwax.expect("deadwax segment");

        assert_eq!(extent.claim, Some(*b"SIDE"));
        assert!(!extent.is_free());
        assert_eq!(extent.free_byte_capacity(), 36_000);
    }

    /// A claim longer than the band it claims is a malformed record, not a
    /// band that happens to be over-full: the writer got its own arithmetic
    /// wrong and every reader after it would inherit the mistake.
    #[test]
    fn a_claim_cannot_outrun_its_band() {
        let mut input = base_input();
        input.deadwax = Some(deadwax(Some(*b"SIDE"), 48_001));

        assert!(encode_record_descriptor_stream(1.0, &input, 4096).is_err());
    }

    /// A record whose programme ran to the label has no band, and writes no
    /// segment: absence is the declaration.
    #[test]
    fn no_deadwax_writes_no_segment() {
        let bytes = encode_record_descriptor_stream(1.0, &base_input(), 4096).expect("stream");
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).expect("decode");

        assert!(decoded.deadwax.is_none());
    }

    #[test]
    fn a_clockwise_cut_writes_no_handedness_segment() {
        // The hand every record already carries costs nothing to say, so it
        // is not said: an ordinary cut's bytes must not move.
        let bytes = encode_record_descriptor_stream(1.0, &base_input(), 4096).expect("stream");
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).expect("decode");

        assert!(decoded.spiral_clockwise);
        assert!(!bytes.contains(&record_descriptor::SEGMENT_GROOVE_HANDEDNESS));
    }

    /// The run-out is derived from the prefix rather than described on the
    /// wire, so the numbers that drew it are part of the format. Every record
    /// says which revision of them it was cut under, and this build reads
    /// back the one it writes.
    #[test]
    fn every_record_declares_the_geometry_its_run_out_was_drawn_by() {
        let bytes = encode_record_descriptor_stream(1.0, &base_input(), 4096).expect("stream");
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).expect("decode");

        assert_eq!(
            decoded.lead_out_geometry_revision,
            record_descriptor::LEAD_OUT_GEOMETRY_REVISION,
        );
        assert!(bytes.contains(&record_descriptor::SEGMENT_LEAD_OUT_GEOMETRY));
    }

    #[test]
    fn a_lathe_cut_carries_its_hand_to_the_reader() {
        // A groove cut the way a lathe cuts one — anticlockwise inward,
        // because the platter turns clockwise under a head that does not
        // travel — has to say so, or a reader retraces the mirror of it and
        // lifts the pixels in the wrong order.
        let mut input = base_input();
        input.spiral_anticlockwise = true;

        let bytes = encode_record_descriptor_stream(1.0, &input, 4096).expect("stream");
        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).expect("decode");

        assert!(!decoded.spiral_clockwise);
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
        assert_eq!(bytes[4], RECORD_DESCRIPTOR_VERSION_HOUSE);

        let decoded = record_descriptor::decode_record_descriptor_bytes(&bytes).unwrap();
        assert_eq!(decoded.version, RECORD_DESCRIPTOR_VERSION_HOUSE);
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
