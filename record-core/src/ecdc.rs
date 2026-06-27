//! BRS1 integration helpers for opaque ECDC payload entries.
//!
//! `record-core` does not parse, validate, or decode ECDC codec packets.
//! Standalone ECDC envelope handling is delegated to `encodec-rs`; packet
//! framing, CRCs, arithmetic coding, model compatibility, and decodability
//! remain owned by that crate.
//!
//! This module owns only:
//!
//! - conversion between the standalone ECDC envelope and the BRS1 shared
//!   payload descriptor plus opaque payload bytes;
//! - construction and validation of the BRS1-visible ECDC descriptor;
//! - logical programme duration from the descriptor's `output_samples`.
//!
//! Each ECDC payload entry represents one programme-time revolution. That is a
//! temporal programme unit, not one geometric turn of the PNG raster spiral.

use anyhow::{bail, Context, Result};
use encodec_rs::binary::{prepend_ecdc_header, split_ecdc_header};
use serde::Serialize;
use serde_json::Value;

use crate::{PayloadDescriptor, PAYLOAD_CONTAINER_ECDC};

/// Decoded-block output geometry for the current fixed EnCodec programme unit.
///
/// These constants describe the current Bitneedle integration profile. They do
/// not describe ECDC compressed packet framing.
pub const ECDC_BLOCK_SAMPLES: u32 = 64_960;
pub const ECDC_OUTPUT_OFFSET_SAMPLES: u32 = 480;
pub const ECDC_OUTPUT_SAMPLES: u32 = 64_000;

/// Codec-specific ECDC metadata stored once in the shared BRS1 descriptor.
///
/// Field declaration order defines the compact JSON byte order produced by
/// [`ecdc_codec_metadata_json`]. BRS1 programme duration comes from the typed
/// descriptor output geometry, not from `"fl"` or compressed packet count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EcdcCodecMetadata {
    #[serde(rename = "m")]
    pub model: String,
    #[serde(rename = "nc")]
    pub num_codebooks: u32,
    #[serde(rename = "lm")]
    pub lm: bool,
    #[serde(rename = "fp")]
    pub fp_scale: u32,
    #[serde(rename = "mr")]
    pub min_range: u32,
    #[serde(rename = "acv")]
    pub bitstream_version: u8,
    #[serde(rename = "fl")]
    pub lm_frame_length: u32,
}

/// Serialize [`EcdcCodecMetadata`] to compact UTF-8 JSON bytes.
pub fn ecdc_codec_metadata_json(metadata: &EcdcCodecMetadata) -> Result<Vec<u8>> {
    serde_json::to_vec(metadata).context("failed to serialize ECDC codec metadata")
}

/// Build the current fixed ECDC BRS1 payload descriptor.
pub fn ecdc_payload_descriptor(
    sample_rate: u32,
    channels: u8,
    codec_metadata: &EcdcCodecMetadata,
) -> Result<PayloadDescriptor> {
    let descriptor = PayloadDescriptor {
        container: PAYLOAD_CONTAINER_ECDC.to_owned(),
        codec: Some(PAYLOAD_CONTAINER_ECDC.to_owned()),
        sample_rate: Some(sample_rate),
        channels: Some(channels),
        block_samples: Some(ECDC_BLOCK_SAMPLES),
        output_offset_samples: Some(ECDC_OUTPUT_OFFSET_SAMPLES),
        output_samples: Some(ECDC_OUTPUT_SAMPLES),
        codec_metadata: Some(ecdc_codec_metadata_json(codec_metadata)?),
    };

    validate_ecdc_descriptor(&descriptor)?;
    Ok(descriptor)
}

/// Split a standalone ECDC stream into raw header JSON and opaque codec bytes.
///
/// Header parsing and envelope validation are delegated to
/// `encodec_rs::binary::split_ecdc_header`. The trailing codec payload is not
/// decoded by `record-core`.
pub fn split_standalone_ecdc(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let parts = split_ecdc_header::<Value>(bytes)
        .context("failed to split standalone ECDC header")?;

    Ok((parts.header_json, parts.payload))
}

/// Lift the standalone ECDC `"al"` field into typed BRS1 descriptor geometry.
///
/// All other header fields are retained as codec metadata. This conversion does
/// not inspect the opaque codec payload bytes.
fn header_json_into_codec_metadata(header_json: &[u8]) -> Result<(u32, Vec<u8>)> {
    let mut object: serde_json::Map<String, Value> =
        serde_json::from_slice(header_json).context("ECDC header is not a JSON object")?;

    let block_samples = object
        .get("al")
        .and_then(Value::as_u64)
        .context("ECDC header is missing numeric \"al\" (audio length)")?;
    let block_samples =
        u32::try_from(block_samples).context("ECDC \"al\" exceeds u32")?;

    object.remove("al");

    let codec_metadata = serde_json::to_vec(&Value::Object(object))
        .context("failed to serialize ECDC codec metadata")?;

    Ok((block_samples, codec_metadata))
}

/// Build a BRS1 ECDC descriptor from standalone ECDC header JSON.
///
/// The standalone `"al"` value becomes the typed block and logical output
/// length with an identity crop. Authoring code may replace this with a narrower
/// crop for a padded integration profile.
pub fn ecdc_payload_descriptor_from_header_json(
    sample_rate: u32,
    channels: u8,
    header_json: &[u8],
) -> Result<PayloadDescriptor> {
    let (block_samples, codec_metadata) =
        header_json_into_codec_metadata(header_json)?;

    let descriptor = PayloadDescriptor {
        container: PAYLOAD_CONTAINER_ECDC.to_owned(),
        codec: Some(PAYLOAD_CONTAINER_ECDC.to_owned()),
        sample_rate: Some(sample_rate),
        channels: Some(channels),
        block_samples: Some(block_samples),
        output_offset_samples: Some(0),
        output_samples: Some(block_samples),
        codec_metadata: Some(codec_metadata),
    };

    validate_ecdc_descriptor(&descriptor)?;
    Ok(descriptor)
}

/// Convert a standalone ECDC stream into a shared BRS1 descriptor and opaque
/// payload bytes.
///
/// `encodec-rs` owns the standalone envelope split. `record-core` only lifts
/// selected header values into the generic BRS1 descriptor.
pub fn standalone_ecdc_to_payload(
    sample_rate: u32,
    channels: u8,
    ecdc_bytes: &[u8],
) -> Result<(PayloadDescriptor, Vec<u8>)> {
    let parts = split_ecdc_header::<Value>(ecdc_bytes)
        .context("failed to split standalone ECDC stream")?;

    if parts.payload.is_empty() {
        bail!("standalone ECDC stream contains no codec payload bytes");
    }

    let descriptor = ecdc_payload_descriptor_from_header_json(
        sample_rate,
        channels,
        parts.header_json,
    )?;

    Ok((descriptor, parts.payload.to_vec()))
}

/// Reconstruct a standalone ECDC stream from a descriptor and opaque payload.
///
/// Envelope reconstruction is delegated to
/// `encodec_rs::binary::prepend_ecdc_header`. The resulting stream may then be
/// fully validated or decoded by `encodec-rs`.
pub fn payload_to_standalone_ecdc(
    descriptor: &PayloadDescriptor,
    payload: &[u8],
) -> Result<Vec<u8>> {
    validate_ecdc_descriptor(descriptor)?;

    if payload.is_empty() {
        bail!("ECDC payload entry must not be empty");
    }

    let codec_metadata = descriptor
        .codec_metadata
        .as_deref()
        .context("ECDC descriptor has no codec metadata")?;
    let block_samples = descriptor
        .block_samples
        .context("ECDC descriptor has no block_samples")?;

    let mut object: serde_json::Map<String, Value> =
        serde_json::from_slice(codec_metadata)
            .context("ECDC codec metadata is not a JSON object")?;
    object.insert("al".to_owned(), Value::from(block_samples));

    let header_json = serde_json::to_vec(&Value::Object(object))
        .context("failed to serialize reconstructed ECDC header")?;

    prepend_ecdc_header(&header_json, payload)
        .context("failed to prepend standalone ECDC header")
}

/// Validate the BRS1-visible fields of an ECDC payload descriptor.
///
/// This does not validate codec metadata against a model bundle and does not
/// inspect ECDC payload-entry bytes.
pub fn validate_ecdc_descriptor(descriptor: &PayloadDescriptor) -> Result<()> {
    if !descriptor
        .container
        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
    {
        bail!(
            "expected ECDC payload container, got `{}`",
            descriptor.container
        );
    }

    match descriptor.codec.as_deref() {
        Some(codec) if codec.eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC) => {}
        Some(codec) => {
            bail!("ECDC payload descriptor codec must be ECDC, got `{codec}`")
        }
        None => bail!("ECDC payload descriptor requires the ECDC codec"),
    }

    match descriptor.sample_rate {
        Some(sample_rate) if sample_rate > 0 => {}
        Some(_) => {
            bail!("ECDC payload descriptor sample rate must be greater than zero")
        }
        None => bail!("ECDC payload descriptor requires a sample rate"),
    }

    match descriptor.channels {
        Some(channels) if channels > 0 => {}
        Some(_) => {
            bail!("ECDC payload descriptor channel count must be greater than zero")
        }
        None => bail!("ECDC payload descriptor requires a channel count"),
    }

    crate::validate_payload_descriptor(descriptor)
        .context("invalid ECDC payload descriptor")?;

    if descriptor.codec_metadata.is_none() {
        bail!("ECDC payload descriptor requires codec metadata");
    }

    Ok(())
}

/// Logical PCM sample count represented by one opaque ECDC payload entry.
///
/// ECDC packet framing is not inspected. The non-empty bytes are retained
/// exactly by BRS1, while programme timing comes from the descriptor's explicit
/// positive `output_samples`.
pub fn headerless_entry_sample_count(
    entry_bytes: &[u8],
    descriptor: &PayloadDescriptor,
) -> Result<u64> {
    validate_ecdc_descriptor(descriptor)?;

    if entry_bytes.is_empty() {
        bail!("ECDC payload entry must not be empty");
    }

    let output_samples = descriptor
        .output_samples
        .filter(|samples| *samples > 0)
        .context("ECDC payload descriptor requires positive output_samples")?;

    Ok(u64::from(output_samples))
}

#[cfg(test)]
mod tests {
    use super::*;
    use encodec_rs::binary::write_ecdc_header;

    fn metadata() -> EcdcCodecMetadata {
        EcdcCodecMetadata {
            model: "encodec_48khz".to_owned(),
            num_codebooks: 8,
            lm: true,
            fp_scale: 8192,
            min_range: 2,
            bitstream_version: 2,
            lm_frame_length: 203,
        }
    }

    fn standalone_ecdc(al: u32, payload: &[u8]) -> Vec<u8> {
        let header = serde_json::json!({
            "m": "encodec_48khz",
            "al": al,
            "nc": 8,
            "lm": true,
            "fp": 8192,
            "mr": 2,
            "acv": 2,
            "fl": 203
        });

        let mut out = Vec::new();
        write_ecdc_header(&mut out, &header).unwrap();
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn fixed_descriptor_uses_explicit_output_geometry() {
        let descriptor =
            ecdc_payload_descriptor(48_000, 2, &metadata()).unwrap();

        assert_eq!(descriptor.block_samples, Some(64_960));
        assert_eq!(descriptor.output_offset_samples, Some(480));
        assert_eq!(descriptor.output_samples, Some(64_000));
    }

    #[test]
    fn opaque_entry_duration_comes_from_output_samples() {
        let descriptor =
            ecdc_payload_descriptor(48_000, 2, &metadata()).unwrap();

        let entry = b"opaque bytes owned by encodec-rs";
        assert_eq!(
            headerless_entry_sample_count(entry, &descriptor).unwrap(),
            64_000
        );
    }

    #[test]
    fn empty_opaque_entry_is_rejected() {
        let descriptor =
            ecdc_payload_descriptor(48_000, 2, &metadata()).unwrap();

        assert!(headerless_entry_sample_count(&[], &descriptor).is_err());
    }

    #[test]
    fn encodec_rs_splits_and_rebuilds_the_envelope() {
        let original =
            standalone_ecdc(64_000, b"opaque codec payload bytes");

        let (descriptor, payload) =
            standalone_ecdc_to_payload(48_000, 2, &original).unwrap();

        assert_eq!(descriptor.block_samples, Some(64_000));
        assert_eq!(descriptor.output_offset_samples, Some(0));
        assert_eq!(descriptor.output_samples, Some(64_000));
        assert_eq!(payload, b"opaque codec payload bytes");

        let rebuilt =
            payload_to_standalone_ecdc(&descriptor, &payload).unwrap();
        let parts = split_ecdc_header::<Value>(&rebuilt).unwrap();

        assert_eq!(
            parts
                .metadata
                .get("al")
                .and_then(Value::as_u64),
            Some(64_000)
        );
        assert_eq!(parts.payload, payload);
    }

    #[test]
    fn malformed_outer_header_is_rejected_by_encodec_rs() {
        assert!(split_standalone_ecdc(b"not an ECDC stream").is_err());
    }

    #[test]
    fn opaque_payload_contents_are_not_interpreted() {
        let original = standalone_ecdc(
            64_000,
            &[0xff, 0x00, 0x13, 0x37, 0x01],
        );

        let (_, payload) =
            standalone_ecdc_to_payload(48_000, 2, &original).unwrap();

        assert_eq!(payload, vec![0xff, 0x00, 0x13, 0x37, 0x01]);
    }
}
