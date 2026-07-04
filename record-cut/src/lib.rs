// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

#![doc = include_str!("../README.md")]

//! Canonical Bitneedle BRS1 record-stream creation and encoding.
//!
//! This crate is canonical-output only:
//! - no JSON wire metadata;
//! - no decoder fork;
//! - no legacy compatibility wrappers.
//!
//! Public parsing, verification, geometry, and decoding belong in `record-core`.
//!
//! ECDC payloads remain opaque record payload bytes. This crate preserves each
//! ECDC payload entry as one BRS1 chunk and does not parse or split codec
//! frames.

use anyhow::{bail, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
pub use record_core::{chunk_encryption_aad, crc32_ieee};
use record_core::{
    validate_payload_descriptor, PayloadDescriptor, CHUNK_CRC32_LENGTH,
    CHUNK_ENCRYPTION_KEY_LENGTH, CHUNK_ENCRYPTION_NONCE_LENGTH, CHUNK_ENCRYPTION_TAG_LENGTH,
    CONTAINER_ECDC, CONTAINER_EXTENSION, CONTAINER_MOSS_NANO, DESCRIPTOR_FLAG_CHANNELS,
    DESCRIPTOR_FLAG_CODEC, DESCRIPTOR_FLAG_CODEC_METADATA, DESCRIPTOR_FLAG_OUTPUT_GEOMETRY,
    DESCRIPTOR_FLAG_SAMPLE_RATE, MAX_CHUNKS, MAX_CODEC_METADATA_BYTES, METADATA_FLAG_ENCRYPTED,
    METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES, METADATA_FLAG_TRACK_ENTRY_MAPPINGS,
    METADATA_FLAG_TRACK_GAPS, PAYLOAD_CONTAINER_ECDC, RECORD_STREAM_MAGIC,
    RECORD_STREAM_METADATA_VERSION,
};

pub mod descriptor;
pub mod gap;

const MAX_WIRE_STRING_BYTES: usize = u16::MAX as usize;
const DEFAULT_EXTENSION_PAYLOAD_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordStreamInput {
    pub payload_descriptors: Vec<PayloadDescriptorInput>,
    pub tracks: Vec<TrackInput>,
    pub track_gaps: Vec<TrackGapInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PayloadDescriptorInput {
    pub container: String,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub block_samples: Option<u32>,
    pub output_offset_samples: Option<u32>,
    pub output_samples: Option<u32>,
    pub codec_metadata: Option<Vec<u8>>,
}

impl PayloadDescriptorInput {
    /// A descriptor that carries only a container name.
    pub fn from_container(container: impl Into<String>) -> Self {
        Self {
            container: container.into(),
            ..Self::default()
        }
    }

    /// Build the fixed EnCodec revolution descriptor from an extracted ECDC
    /// header. The decoded-block output geometry comes from the shared
    /// `record_core::ecdc` profile constants; `codec_metadata` carries the
    /// codec-specific portion of the header as canonical compact JSON.
    pub fn ecdc(
        sample_rate: u32,
        channels: u8,
        codec_metadata: &record_core::ecdc::EcdcCodecMetadata,
    ) -> Result<Self> {
        Ok(Self::from(record_core::ecdc::ecdc_payload_descriptor(
            sample_rate,
            channels,
            codec_metadata,
        )?))
    }

    fn to_descriptor(&self) -> PayloadDescriptor {
        PayloadDescriptor {
            container: self.container.clone(),
            codec: self.codec.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            block_samples: self.block_samples,
            output_offset_samples: self.output_offset_samples,
            output_samples: self.output_samples,
            codec_metadata: self.codec_metadata.clone(),
        }
    }
}

impl From<PayloadDescriptor> for PayloadDescriptorInput {
    fn from(descriptor: PayloadDescriptor) -> Self {
        Self {
            container: descriptor.container,
            codec: descriptor.codec,
            sample_rate: descriptor.sample_rate,
            channels: descriptor.channels,
            block_samples: descriptor.block_samples,
            output_offset_samples: descriptor.output_offset_samples,
            output_samples: descriptor.output_samples,
            codec_metadata: descriptor.codec_metadata,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInput {
    pub title: String,

    /// Omit when this track is a single revolution at the same array
    /// position as its index (the common one-track-per-entry case).
    pub first_revolution_index: Option<usize>,
    pub revolution_count: Option<usize>,
}

/// An explicit inter-track programme gap: a consecutive range of payload
/// entries (ordinary entries, ECDC in practice — never a special GAP
/// container) that is intentionally not part of any track.
/// `after_track_index` is the 0-based index into `RecordStreamInput::tracks`
/// this gap immediately follows. Every payload entry must belong to exactly
/// one track or track gap; there is no implicit "uncovered means gap"
/// fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackGapInput {
    pub first_revolution_index: usize,
    pub revolution_count: usize,
    pub after_track_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadEntryInput {
    pub payload_descriptor_index: u8,
    pub bytes: Vec<u8>,
}

impl PayloadEntryInput {
    pub fn new(payload_descriptor_index: u8, bytes: Vec<u8>) -> Self {
        Self {
            payload_descriptor_index,
            bytes,
        }
    }

    /// Compatibility constructor. Chunking is selected internally from the
    /// payload descriptor's container.
    pub fn container_default(payload_descriptor_index: u8, bytes: Vec<u8>) -> Self {
        Self::new(payload_descriptor_index, bytes)
    }

    /// Compatibility constructor for callers that already provide one logical
    /// payload entry, preserved as one BRS1 chunk. Track and track-gap
    /// entries alike are ordinary payload entries (ECDC in practice); there
    /// is no special gap container or constructor.
    pub fn already_chunked(payload_descriptor_index: u8, bytes: Vec<u8>) -> Self {
        Self::new(payload_descriptor_index, bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRecordStream {
    pub metadata_bytes: Vec<u8>,
    pub payloads: Vec<Vec<u8>>,
    pub payload_descriptor_indexes: Vec<u8>,
    pub inspection: RecordStreamInspection,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStreamInspection {
    pub format: &'static str,
    pub version: u8,
    pub encrypted: bool,
    pub metadata_byte_length: usize,
    pub payload_byte_length: usize,
    pub payload_entry_count: usize,
    pub chunk_count: usize,
    pub payload_descriptors: Vec<PayloadDescriptorInspection>,
    pub payload_entries: Vec<PayloadEntryInspection>,
    pub tracks: Vec<TrackInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadDescriptorInspection {
    pub index: usize,
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_offset_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_metadata_byte_length: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadEntryInspection {
    pub index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub payload_descriptor_index: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInspection {
    pub number: usize,
    pub title: String,
    pub first_revolution_index: usize,
    pub revolution_count: usize,
}

fn push_u16be(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
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

fn push_string(out: &mut Vec<u8>, value: &str, label: &str) -> Result<()> {
    let bytes = value.as_bytes();

    if bytes.len() > MAX_WIRE_STRING_BYTES {
        bail!("{label} exceeds u16 string length limit");
    }

    push_u16be(out, bytes.len() as u16);
    out.extend_from_slice(bytes);
    Ok(())
}

fn normalized_required_text(value: &str, label: &str) -> Result<String> {
    let normalized = value.trim();

    if normalized.is_empty() {
        bail!("{label} must not be empty");
    }

    if normalized.chars().any(char::is_control) {
        bail!("{label} must not contain control characters");
    }

    Ok(normalized.to_owned())
}

fn container_code(container: &str) -> (u8, Option<&str>) {
    if container.eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC) {
        (CONTAINER_ECDC, None)
    } else if container.eq_ignore_ascii_case("MOSSNANO") {
        (CONTAINER_MOSS_NANO, None)
    } else {
        (CONTAINER_EXTENSION, Some(container))
    }
}

fn payload_total(entries: &[PayloadEntryInput]) -> Result<usize> {
    entries.iter().try_fold(0usize, |total, entry| {
        total
            .checked_add(entry.bytes.len())
            .context("payload byte length overflow")
    })
}

fn validate_inputs(input: &RecordStreamInput, entries: &[PayloadEntryInput]) -> Result<()> {
    if input.payload_descriptors.is_empty() {
        bail!("record stream must contain at least one payload descriptor");
    }

    if input.payload_descriptors.len() > u8::MAX as usize {
        bail!("payload descriptor count exceeds u8 index range");
    }

    if entries.is_empty() {
        bail!("record stream must contain at least one payload entry");
    }

    if input.tracks.is_empty() {
        bail!("record stream must contain at least one track");
    }

    for (index, descriptor) in input.payload_descriptors.iter().enumerate() {
        normalized_required_text(
            &descriptor.container,
            &format!("payload descriptor {index} container"),
        )?;
        validate_payload_descriptor(&descriptor.to_descriptor())
            .with_context(|| format!("payload descriptor {index} is invalid"))?;
    }

    for (index, entry) in entries.iter().enumerate() {
        if entry.bytes.is_empty() {
            bail!("payload entry {index} is empty");
        }

        if entry.payload_descriptor_index as usize >= input.payload_descriptors.len() {
            bail!(
                "payload entry {index} descriptor index {} is out of range for {} descriptors",
                entry.payload_descriptor_index,
                input.payload_descriptors.len()
            );
        }
    }

    let mut track_ranges = Vec::with_capacity(input.tracks.len());

    for (index, track) in input.tracks.iter().enumerate() {
        normalized_required_text(&track.title, &format!("track {index} title"))?;

        let first = track.first_revolution_index.unwrap_or(index);
        let count = track.revolution_count.unwrap_or(1);

        if count == 0 {
            bail!("track {index} revolution count must be greater than zero");
        }

        let end = first
            .checked_add(count)
            .context("track revolution range overflows")?;

        if end > entries.len() {
            bail!(
                "track {index} revolution range [{first}, {end}) is out of range for {} entries",
                entries.len()
            );
        }

        track_ranges.push(record_core::tracks::TrackRange {
            title: track.title.clone(),
            first_revolution_index: first as u64,
            revolution_count: count as u64,
        });
    }

    let mut gap_ranges = Vec::with_capacity(input.track_gaps.len());

    for (index, gap) in input.track_gaps.iter().enumerate() {
        if gap.revolution_count == 0 {
            bail!("track gap {index} revolution count must be greater than zero");
        }

        if gap.after_track_index >= input.tracks.len() {
            bail!(
                "track gap {index} after_track_index {} is out of range for {} tracks",
                gap.after_track_index,
                input.tracks.len()
            );
        }

        let end = gap
            .first_revolution_index
            .checked_add(gap.revolution_count)
            .context("track gap revolution range overflows")?;

        if end > entries.len() {
            bail!(
                "track gap {index} revolution range [{}, {end}) is out of range for {} entries",
                gap.first_revolution_index,
                entries.len()
            );
        }

        if let Some(previous) = input.track_gaps.get(index.wrapping_sub(1)) {
            if index > 0 && gap.first_revolution_index <= previous.first_revolution_index {
                bail!(
                    "track gap {index} first_revolution_index {} is not strictly ascending after the previous track gap's {}",
                    gap.first_revolution_index,
                    previous.first_revolution_index
                );
            }
        }

        gap_ranges.push(record_core::tracks::TrackGapRange {
            first_revolution_index: gap.first_revolution_index as u64,
            revolution_count: gap.revolution_count as u64,
            after_track_index: gap.after_track_index,
        });
    }

    // Every payload entry must belong to exactly one explicit track or
    // track-gap range. There is no implicit "uncovered means gap" fallback:
    // an uncovered, overlapping, or doubly-covered entry is rejected here,
    // before serialization, by the same coverage rule record-core enforces
    // on read.
    let is_playable_revolution = vec![true; entries.len()];
    record_core::tracks::validate_track_ranges(
        &track_ranges,
        &gap_ranges,
        &is_playable_revolution,
    )?;

    Ok(())
}

fn encode_metadata(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
    encrypted: bool,
) -> Result<Vec<u8>> {
    validate_inputs(input, entries)?;

    let has_entry_descriptor_indexes = entries
        .iter()
        .any(|entry| entry.payload_descriptor_index != 0);

    let has_track_entry_mappings = input.tracks.iter().enumerate().any(|(index, track)| {
        track.first_revolution_index.unwrap_or(index) != index
            || track.revolution_count.unwrap_or(1) != 1
    });

    let mut flags = 0u8;

    if encrypted {
        flags |= METADATA_FLAG_ENCRYPTED;
    }

    if has_entry_descriptor_indexes {
        flags |= METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES;
    }

    if has_track_entry_mappings {
        flags |= METADATA_FLAG_TRACK_ENTRY_MAPPINGS;
    }

    if !input.track_gaps.is_empty() {
        flags |= METADATA_FLAG_TRACK_GAPS;
    }

    let descriptor_count = u8::try_from(input.payload_descriptors.len())
        .context("payload descriptor count exceeds u8")?;
    let entry_count = u16::try_from(entries.len()).context("payload entry count exceeds u16")?;
    let track_count = u16::try_from(input.tracks.len()).context("track count exceeds u16")?;

    let mut out = Vec::new();
    out.push(RECORD_STREAM_METADATA_VERSION);
    out.push(flags);
    out.push(descriptor_count);

    for (index, descriptor) in input.payload_descriptors.iter().enumerate() {
        let container = normalized_required_text(
            &descriptor.container,
            &format!("descriptor {index} container"),
        )?;

        let (code, extension_name) = container_code(&container);
        out.push(code);

        if let Some(extension_name) = extension_name {
            push_string(&mut out, extension_name, "extension container name")?;
        }

        let mut flags = 0u8;
        if descriptor.codec.is_some() {
            flags |= DESCRIPTOR_FLAG_CODEC;
        }
        if descriptor.sample_rate.is_some() {
            flags |= DESCRIPTOR_FLAG_SAMPLE_RATE;
        }
        if descriptor.channels.is_some() {
            flags |= DESCRIPTOR_FLAG_CHANNELS;
        }
        if descriptor.block_samples.is_some() {
            flags |= DESCRIPTOR_FLAG_OUTPUT_GEOMETRY;
        }
        if descriptor.codec_metadata.is_some() {
            flags |= DESCRIPTOR_FLAG_CODEC_METADATA;
        }
        out.push(flags);

        if let Some(codec) = &descriptor.codec {
            let codec = normalized_required_text(codec, &format!("descriptor {index} codec"))?;
            push_string(&mut out, &codec, "descriptor codec")?;
        }
        if let Some(sample_rate) = descriptor.sample_rate {
            push_u32be(&mut out, sample_rate);
        }
        if let Some(channels) = descriptor.channels {
            out.push(channels);
        }
        // The three geometry fields are signalled together by a single flag and
        // are guaranteed present together by validate_payload_descriptor above.
        if let Some(block_samples) = descriptor.block_samples {
            push_u32be(&mut out, block_samples);
            push_u32be(
                &mut out,
                descriptor
                    .output_offset_samples
                    .expect("validated geometry present"),
            );
            push_u32be(
                &mut out,
                descriptor
                    .output_samples
                    .expect("validated geometry present"),
            );
        }
        if let Some(codec_metadata) = &descriptor.codec_metadata {
            if codec_metadata.len() > MAX_CODEC_METADATA_BYTES {
                bail!(
                    "descriptor {index} codec metadata length {} exceeds limit {MAX_CODEC_METADATA_BYTES}",
                    codec_metadata.len()
                );
            }
            push_u32be(
                &mut out,
                u32::try_from(codec_metadata.len()).context("codec metadata length exceeds u32")?,
            );
            out.extend_from_slice(codec_metadata);
        }
    }

    push_u16be(&mut out, entry_count);

    for entry in entries {
        push_varuint(
            &mut out,
            u64::try_from(entry.bytes.len()).context("payload entry length exceeds u64")?,
        );

        if has_entry_descriptor_indexes {
            out.push(entry.payload_descriptor_index);
        }
    }

    push_u16be(&mut out, track_count);

    for (index, track) in input.tracks.iter().enumerate() {
        let title = normalized_required_text(&track.title, &format!("track {index} title"))?;
        push_string(&mut out, &title, "track title")?;

        if has_track_entry_mappings {
            let first = track.first_revolution_index.unwrap_or(index);
            let count = track.revolution_count.unwrap_or(1);
            push_varuint(
                &mut out,
                u64::try_from(first).context("track first revolution index exceeds u64")?,
            );
            push_varuint(
                &mut out,
                u64::try_from(count).context("track revolution count exceeds u64")?,
            );
        }
    }

    if !input.track_gaps.is_empty() {
        let track_gap_count =
            u16::try_from(input.track_gaps.len()).context("track gap count exceeds u16")?;
        push_u16be(&mut out, track_gap_count);

        for (index, gap) in input.track_gaps.iter().enumerate() {
            if let Some(previous) = index.checked_sub(1).and_then(|i| input.track_gaps.get(i)) {
                if gap.first_revolution_index <= previous.first_revolution_index {
                    bail!(
                        "track gap {index} first_revolution_index {} is not strictly ascending after the previous track gap's {}",
                        gap.first_revolution_index,
                        previous.first_revolution_index
                    );
                }
            }

            push_varuint(
                &mut out,
                u64::try_from(gap.first_revolution_index)
                    .context("track gap first revolution index exceeds u64")?,
            );
            push_varuint(
                &mut out,
                u64::try_from(gap.revolution_count)
                    .context("track gap revolution count exceeds u64")?,
            );
            push_varuint(
                &mut out,
                u64::try_from(gap.after_track_index)
                    .context("track gap after track index exceeds u64")?,
            );
        }
    }

    Ok(out)
}

fn build_inspection(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
    payloads: &[Vec<u8>],
    metadata_byte_length: usize,
    encrypted: bool,
) -> Result<RecordStreamInspection> {
    let mut byte_offset = 0usize;
    let mut payload_entries = Vec::with_capacity(entries.len());

    for (index, entry) in entries.iter().enumerate() {
        payload_entries.push(PayloadEntryInspection {
            index,
            byte_offset,
            byte_length: entry.bytes.len(),
            payload_descriptor_index: entry.payload_descriptor_index,
        });

        byte_offset = byte_offset
            .checked_add(entry.bytes.len())
            .context("payload entry byte offset overflow")?;
    }

    Ok(RecordStreamInspection {
        format: "bitneedle-record-stream",
        version: 2,
        encrypted,
        metadata_byte_length,
        payload_byte_length: payload_total(entries)?,
        payload_entry_count: entries.len(),
        chunk_count: payloads.len(),
        payload_descriptors: input
            .payload_descriptors
            .iter()
            .enumerate()
            .map(|(index, descriptor)| PayloadDescriptorInspection {
                index,
                container: descriptor.container.trim().to_owned(),
                codec: descriptor.codec.clone(),
                sample_rate: descriptor.sample_rate,
                channels: descriptor.channels,
                block_samples: descriptor.block_samples,
                output_offset_samples: descriptor.output_offset_samples,
                output_samples: descriptor.output_samples,
                codec_metadata_byte_length: descriptor.codec_metadata.as_ref().map(Vec::len),
            })
            .collect(),
        payload_entries,
        tracks: input
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| TrackInspection {
                number: index + 1,
                title: track.title.trim().to_owned(),
                first_revolution_index: track.first_revolution_index.unwrap_or(index),
                revolution_count: track.revolution_count.unwrap_or(1),
            })
            .collect(),
    })
}

pub fn plan_record_stream(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
) -> Result<PlannedRecordStream> {
    plan_record_stream_internal(input, entries, false)
}

pub fn plan_encrypted_record_stream(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
) -> Result<PlannedRecordStream> {
    plan_record_stream_internal(input, entries, true)
}

fn plan_record_stream_internal(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
    encrypted: bool,
) -> Result<PlannedRecordStream> {
    validate_inputs(input, entries)?;
    let metadata_bytes = encode_metadata(input, entries, encrypted)?;
    let mut payloads = Vec::new();
    let mut payload_descriptor_indexes = Vec::new();

    for entry in entries {
        let descriptor = &input.payload_descriptors[entry.payload_descriptor_index as usize];
        let chunks = payload_chunks_for_container(&descriptor.container, &entry.bytes)?;

        for chunk in chunks {
            payloads.push(chunk);
            payload_descriptor_indexes.push(entry.payload_descriptor_index);
        }
    }

    if payloads.len() > MAX_CHUNKS {
        bail!("chunk count exceeds u16 range");
    }

    let inspection = build_inspection(input, entries, &payloads, metadata_bytes.len(), encrypted)?;

    Ok(PlannedRecordStream {
        metadata_bytes,
        payloads,
        payload_descriptor_indexes,
        inspection,
    })
}

/// Byte length of a canonical LEB128 varuint encoding of `value`.
fn varuint_byte_length(value: usize) -> usize {
    let mut value = value as u64;
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

pub fn signed_chunk_section_byte_length(payloads: &[Vec<u8>]) -> Result<usize> {
    payloads.iter().try_fold(0usize, |total, payload| {
        total
            .checked_add(varuint_byte_length(payload.len()))
            .and_then(|value| value.checked_add(CHUNK_CRC32_LENGTH))
            .and_then(|value| value.checked_add(payload.len()))
            .context("signed BRS1 chunk section length overflow")
    })
}

pub fn encrypted_chunk_section_byte_length(payloads: &[Vec<u8>]) -> Result<usize> {
    payloads.iter().try_fold(0usize, |total, payload| {
        let ciphertext_len = payload.len() + CHUNK_ENCRYPTION_TAG_LENGTH;
        total
            .checked_add(varuint_byte_length(ciphertext_len))
            .and_then(|value| value.checked_add(CHUNK_CRC32_LENGTH))
            .and_then(|value| value.checked_add(CHUNK_ENCRYPTION_NONCE_LENGTH))
            .and_then(|value| value.checked_add(ciphertext_len))
            .context("encrypted BRS1 chunk section length overflow")
    })
}

pub fn encode_record_stream(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
) -> Result<Vec<u8>> {
    let plan = plan_record_stream(input, entries)?;
    encode_signed_plan(&plan)
}

pub fn encode_encrypted_record_stream(
    input: &RecordStreamInput,
    entries: &[PayloadEntryInput],
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
    nonces: &[[u8; CHUNK_ENCRYPTION_NONCE_LENGTH]],
) -> Result<Vec<u8>> {
    let plan = plan_encrypted_record_stream(input, entries)?;
    encode_encrypted_plan(&plan, key, nonces)
}

pub fn encode_signed_plan(plan: &PlannedRecordStream) -> Result<Vec<u8>> {
    validate_plan(plan)?;

    let metadata_len = u32::try_from(plan.metadata_bytes.len()).context("metadata exceeds u32")?;

    let mut out = Vec::new();
    out.extend_from_slice(RECORD_STREAM_MAGIC);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(&plan.metadata_bytes);

    for payload in &plan.payloads {
        out.extend_from_slice(&record_core::chunk::encode_unencrypted_chunk(payload)?);
    }

    Ok(out)
}

pub fn encode_encrypted_plan(
    plan: &PlannedRecordStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
    nonces: &[[u8; CHUNK_ENCRYPTION_NONCE_LENGTH]],
) -> Result<Vec<u8>> {
    validate_plan(plan)?;

    if !plan.inspection.encrypted {
        bail!("record stream plan is not marked encrypted");
    }

    if nonces.len() != plan.payloads.len() {
        bail!("nonce count must match payload chunk count");
    }

    let metadata_len = u32::try_from(plan.metadata_bytes.len()).context("metadata exceeds u32")?;
    let aad = chunk_encryption_aad(&plan.metadata_bytes)?;

    let mut out = Vec::new();
    out.extend_from_slice(RECORD_STREAM_MAGIC);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(&plan.metadata_bytes);

    for (position, payload) in plan.payloads.iter().enumerate() {
        let nonce = nonces[position];
        let encrypted = encrypt_chunk_payload_chacha20poly1305(key, &nonce, &aad, payload)?;
        out.extend_from_slice(&record_core::chunk::encode_encrypted_chunk(
            nonce, &encrypted,
        )?);
    }

    Ok(out)
}

fn validate_plan(plan: &PlannedRecordStream) -> Result<()> {
    if plan.payloads.is_empty() {
        bail!("record stream plan contains no chunks");
    }

    if plan.payloads.len() > MAX_CHUNKS {
        bail!("chunk count exceeds u16 range");
    }

    if plan.payload_descriptor_indexes.len() != plan.payloads.len() {
        bail!("descriptor index count does not match chunk count");
    }

    Ok(())
}

pub fn encrypt_chunk_payload_chacha20poly1305(
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(
            Nonce::from_slice(nonce),
            AeadPayload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("failed to encrypt chunk payload"))
}

pub fn payload_chunks_for_container(
    payload_container: &str,
    payload: &[u8],
) -> Result<Vec<Vec<u8>>> {
    if payload_container.eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC) {
        // ECDC is opaque at this layer. One payload entry becomes one BRS1
        // chunk; record-cut neither removes the standalone header nor splits
        // internal codec packets.
        return Ok(vec![payload.to_vec()]);
    }

    Ok(payload
        .chunks(DEFAULT_EXTENSION_PAYLOAD_CHUNK_SIZE)
        .map(ToOwned::to_owned)
        .collect())
}

pub fn ecdc_frame_aligned_payload_chunks(ecdc: &[u8]) -> Result<Vec<Vec<u8>>> {
    if ecdc.is_empty() {
        bail!("ECDC payload is empty");
    }

    // Retained as a compatibility API. ECDC payload entries are opaque and
    // deliberately preserved as a single BRS1 chunk.
    Ok(vec![ecdc.to_vec()])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CONTAINER: &str = "TEST";

    #[test]
    fn compact_metadata_bytes_are_frozen() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: b"payload".to_vec(),
        }];

        let plan = plan_record_stream(&input, &entries).unwrap();

        // version, flags, descriptor_count
        // descriptor 0: extension container "TEST", descriptor_flags=0
        // entry_count=1 (u16be), entry 0 byte_length=7 (varuint)
        // track_count=1 (u16be), track 0 title length=3 (u16be) + "One"
        let expected: Vec<u8> = vec![
            RECORD_STREAM_METADATA_VERSION,
            0x00, // flags: no encryption, no entry descriptor indexes, no track mappings
            0x01, // descriptor_count
            CONTAINER_EXTENSION,
            0x00,
            0x04, // extension container name length = 4
            b'T',
            b'E',
            b'S',
            b'T',
            0x00, // descriptor_flags
            0x00,
            0x01, // entry_count = 1
            0x07, // entry 0 byte_length varuint = 7
            0x00,
            0x01, // track_count = 1
            0x00,
            0x03, // title length = 3
            b'O',
            b'n',
            b'e',
        ];

        assert_eq!(plan.metadata_bytes, expected);
    }

    #[test]
    fn two_track_round_trip_reconstructs_entries() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: vec![0xAA; 11],
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: vec![0xBB; 13],
            },
        ];

        let bytes = encode_record_stream(&input, &entries).unwrap();

        let parsed = record_core::parse_record_stream(&bytes).unwrap();

        assert_eq!(
            parsed.metadata_bytes,
            plan_record_stream(&input, &entries).unwrap().metadata_bytes
        );
        assert_eq!(parsed.metadata.tracks.len(), 2);
        assert_eq!(parsed.metadata.tracks[0].title, "Side A");
        assert_eq!(parsed.metadata.tracks[1].title, "Side B");
        assert_eq!(parsed.metadata.tracks[0].first_revolution_index, 0);
        assert_eq!(parsed.metadata.tracks[1].first_revolution_index, 1);

        let resolved = record_core::resolve_payload_entries(
            &parsed.metadata.payload_entries,
            parsed.metadata.payload_descriptors.len(),
        )
        .unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].byte_offset, 0);
        assert_eq!(resolved[0].byte_length, 11);
        assert_eq!(resolved[1].byte_offset, 11);
        assert_eq!(resolved[1].byte_length, 13);

        let reconstructed_payload = record_core::record_stream_payload_bytes(&parsed);
        let mut expected_payload = entries[0].bytes.clone();
        expected_payload.extend_from_slice(&entries[1].bytes);

        assert_eq!(reconstructed_payload, expected_payload);
        for (entry, original) in resolved.iter().zip(entries.iter()) {
            assert_eq!(
                &reconstructed_payload[entry.byte_offset..entry.byte_offset + entry.byte_length],
                original.bytes.as_slice()
            );
        }
    }

    #[test]
    fn track_gap_wire_metadata_round_trips_canonically() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput::new(0, vec![0xAA; 11]),
            PayloadEntryInput::new(0, vec![0xCC; 5]),
            PayloadEntryInput::new(0, vec![0xBB; 13]),
        ];

        let bytes = encode_record_stream(&input, &entries).unwrap();
        let parsed = record_core::parse_record_stream(&bytes).unwrap();

        assert_eq!(parsed.metadata.track_gaps.len(), 1);
        assert_eq!(parsed.metadata.track_gaps[0].first_revolution_index, 1);
        assert_eq!(parsed.metadata.track_gaps[0].revolution_count, 1);
        assert_eq!(parsed.metadata.track_gaps[0].after_track_index, 0);

        // Decoding twice is identical (deterministic, canonical encoding).
        let parsed_again = record_core::parse_record_stream(&bytes).unwrap();
        assert_eq!(parsed.metadata, parsed_again.metadata);

        record_core::validate_track_listing_metadata(&parsed.metadata).unwrap();
    }

    #[test]
    fn out_of_order_track_gaps_are_rejected_at_encode_time() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![TrackInput {
                title: "Side A".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(1),
            }],
            track_gaps: vec![
                TrackGapInput {
                    first_revolution_index: 2,
                    revolution_count: 1,
                    after_track_index: 0,
                },
                TrackGapInput {
                    first_revolution_index: 1,
                    revolution_count: 1,
                    after_track_index: 0,
                },
            ],
        };
        let entries = vec![
            PayloadEntryInput::new(0, vec![0xAA; 11]),
            PayloadEntryInput::new(0, vec![0xCC; 5]),
            PayloadEntryInput::new(0, vec![0xCC; 5]),
        ];

        let err = encode_record_stream(&input, &entries).unwrap_err();
        assert!(err.to_string().contains("ascending"));
    }

    #[test]
    fn no_gap_container_descriptor_is_produced_or_accepted() {
        // The new authoring path has no GAP/GAP1 concept at all: an entry
        // declared as a track gap is an ordinary descriptor (TEST_CONTAINER
        // here, ECDC in production) and the encoded metadata never contains
        // a GAP payload container.
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput::new(0, vec![0xAA; 11]),
            PayloadEntryInput::new(0, vec![0xCC; 5]),
            PayloadEntryInput::new(0, vec![0xBB; 13]),
        ];

        let bytes = encode_record_stream(&input, &entries).unwrap();
        let parsed = record_core::parse_record_stream(&bytes).unwrap();

        for descriptor in &parsed.metadata.payload_descriptors {
            assert_ne!(descriptor.container, "GAP");
        }
    }

    #[test]
    fn ecdc_entries_pass_through_unchanged() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(
                PAYLOAD_CONTAINER_ECDC,
            )],
            tracks: vec![TrackInput {
                title: "Side A".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(2),
            }],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x22; 7]),
        ];

        let bytes = encode_record_stream(&input, &entries).unwrap();
        let parsed = record_core::parse_record_stream(&bytes).unwrap();
        let resolved = record_core::resolve_payload_entries(
            &parsed.metadata.payload_entries,
            parsed.metadata.payload_descriptors.len(),
        )
        .unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].byte_length, 9);
        assert_eq!(resolved[1].byte_length, 7);
    }

    #[test]
    fn track_gap_entry_is_a_timeline_entry_not_a_track() {
        // The gap entry is an ordinary payload entry under the same
        // descriptor as the track entries — no GAP container, no GAP1
        // payload. It is classified purely by the explicit track_gaps list.
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![
                TrackInput {
                    title: "Track One".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Track Two".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x33; 5]),
            PayloadEntryInput::already_chunked(0, vec![0x22; 7]),
        ];

        let plan = plan_record_stream(&input, &entries).unwrap();
        assert_eq!(plan.payloads.len(), 3);
        assert_eq!(plan.inspection.tracks.len(), 2);
        assert_eq!(plan.inspection.tracks[0].first_revolution_index, 0);
        assert_eq!(plan.inspection.tracks[1].first_revolution_index, 2);
    }

    #[test]
    fn an_uncovered_entry_is_rejected() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![
                TrackInput {
                    title: "Track One".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Track Two".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x33; 5]),
            PayloadEntryInput::already_chunked(0, vec![0x22; 7]),
        ];

        let err = plan_record_stream(&input, &entries).unwrap_err();
        assert!(err
            .to_string()
            .contains("not covered by any track or track gap"));
    }

    #[test]
    fn an_overlapping_track_and_track_gap_is_rejected() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![TrackInput {
                title: "Track One".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(3),
            }],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x33; 5]),
            PayloadEntryInput::already_chunked(0, vec![0x22; 7]),
        ];

        let err = plan_record_stream(&input, &entries).unwrap_err();
        assert!(err.to_string().contains("covered by both"));
    }

    #[test]
    fn overlapping_track_gaps_are_rejected() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![TrackInput {
                title: "Track One".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(1),
            }],
            track_gaps: vec![
                TrackGapInput {
                    first_revolution_index: 1,
                    revolution_count: 2,
                    after_track_index: 0,
                },
                TrackGapInput {
                    first_revolution_index: 2,
                    revolution_count: 1,
                    after_track_index: 0,
                },
            ],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x33; 5]),
            PayloadEntryInput::already_chunked(0, vec![0x22; 7]),
        ];

        let err = plan_record_stream(&input, &entries).unwrap_err();
        assert!(
            err.to_string().contains("ascending")
                || err.to_string().contains("covered by more than one")
        );
    }

    #[test]
    fn zero_length_track_gap_is_rejected() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![
                TrackInput {
                    title: "Track One".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Track Two".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 0,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x33; 5]),
            PayloadEntryInput::already_chunked(0, vec![0x22; 7]),
        ];

        let err = plan_record_stream(&input, &entries).unwrap_err();
        assert!(err
            .to_string()
            .contains("revolution count must be greater than zero"));
    }

    #[test]
    fn invalid_after_track_index_is_rejected() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![TrackInput {
                title: "Track One".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(1),
            }],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 5,
            }],
        };
        let entries = vec![
            PayloadEntryInput::already_chunked(0, vec![0x11; 9]),
            PayloadEntryInput::already_chunked(0, vec![0x33; 5]),
        ];

        let err = plan_record_stream(&input, &entries).unwrap_err();
        assert!(err.to_string().contains("after_track_index"));
    }

    #[test]
    fn metadata_is_binary_not_json() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: b"payload".to_vec(),
        }];

        let plan = plan_record_stream(&input, &entries).unwrap();

        assert_eq!(plan.metadata_bytes[0], RECORD_STREAM_METADATA_VERSION);
        assert_ne!(plan.metadata_bytes[0], b'{');
        assert!(std::str::from_utf8(&plan.metadata_bytes)
            .map(|text| !text.contains("payloadDescriptors"))
            .unwrap_or(true));
    }

    #[test]
    fn positional_fields_are_implicit() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![
                TrackInput {
                    title: "One".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
                TrackInput {
                    title: "Two".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: vec![1; 10],
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: vec![2; 20],
            },
        ];

        let plan = plan_record_stream(&input, &entries).unwrap();

        assert_eq!(
            plan.metadata_bytes[1] & METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES,
            0
        );
        assert_eq!(
            plan.metadata_bytes[1] & METADATA_FLAG_TRACK_ENTRY_MAPPINGS,
            0
        );
        assert_eq!(plan.inspection.payload_entries[1].byte_offset, 10);
        assert_eq!(plan.inspection.tracks[1].number, 2);
        assert_eq!(plan.inspection.tracks[1].first_revolution_index, 1);
    }

    #[test]
    fn encoder_emits_brs1() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: b"payload".to_vec(),
        }];

        let bytes = encode_record_stream(&input, &entries).unwrap();

        assert_eq!(&bytes[..4], b"BRS1");
    }

    #[test]
    fn encrypted_plan_sets_metadata_flag() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput {
                container: TEST_CONTAINER.to_string(),
                ..Default::default()
            }],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: b"secret".to_vec(),
        }];

        let plan = plan_encrypted_record_stream(&input, &entries).unwrap();

        assert_ne!(plan.metadata_bytes[1] & METADATA_FLAG_ENCRYPTED, 0);
        assert!(plan.inspection.encrypted);
    }

    fn ecdc_codec_metadata() -> record_core::ecdc::EcdcCodecMetadata {
        record_core::ecdc::EcdcCodecMetadata {
            model: "encodec_48khz".to_owned(),
            num_codebooks: 8,
            lm: true,
            fp_scale: 8192,
            min_range: 2,
            bitstream_version: 2,
            lm_frame_length: 203,
        }
    }

    #[test]
    fn ecdc_profile_descriptor_values() {
        let descriptor = PayloadDescriptorInput::ecdc(48_000, 2, &ecdc_codec_metadata()).unwrap();
        assert_eq!(descriptor.container, PAYLOAD_CONTAINER_ECDC);
        assert_eq!(descriptor.block_samples, Some(64_960));
        assert_eq!(descriptor.output_offset_samples, Some(480));
        assert_eq!(descriptor.output_samples, Some(64_000));
    }

    /// Cross-repository round-trip: this crate (bitneedle-platform) encodes a
    /// BRS1 stream carrying every new typed descriptor field plus a
    /// `codec_metadata` blob, and the authoritative `record-core` (bitneedle)
    /// decoder parses it back unchanged. An extension container is used so the
    /// round-trip exercises the descriptor wire format without entangling the
    /// ECDC payload-body validation path.
    #[test]
    fn full_descriptor_round_trips_through_record_core() {
        let codec_metadata =
            record_core::ecdc::ecdc_codec_metadata_json(&ecdc_codec_metadata()).unwrap();
        let descriptor = PayloadDescriptorInput {
            container: "TESTCODEC".to_string(),
            codec: Some(PAYLOAD_CONTAINER_ECDC.to_string()),
            sample_rate: Some(48_000),
            channels: Some(2),
            block_samples: Some(64_960),
            output_offset_samples: Some(480),
            output_samples: Some(64_000),
            codec_metadata: Some(codec_metadata.clone()),
        };

        let input = RecordStreamInput {
            payload_descriptors: vec![descriptor],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: vec![0u8; 32],
        }];

        let bytes = encode_record_stream(&input, &entries).unwrap();
        let parsed = record_core::parse_record_stream(&bytes).unwrap();
        let decoded = &parsed.metadata.payload_descriptors[0];

        assert_eq!(decoded.container, "TESTCODEC");
        assert_eq!(decoded.codec.as_deref(), Some(PAYLOAD_CONTAINER_ECDC));
        assert_eq!(decoded.sample_rate, Some(48_000));
        assert_eq!(decoded.channels, Some(2));
        assert_eq!(decoded.block_samples, Some(64_960));
        assert_eq!(decoded.output_offset_samples, Some(480));
        assert_eq!(decoded.output_samples, Some(64_000));
        assert_eq!(decoded.codec_metadata.as_ref(), Some(&codec_metadata));
    }

    #[test]
    fn descriptor_without_geometry_round_trips_through_record_core() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(TEST_CONTAINER)],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: b"payload".to_vec(),
        }];

        let bytes = encode_record_stream(&input, &entries).unwrap();
        let parsed = record_core::parse_record_stream(&bytes).unwrap();
        let decoded = &parsed.metadata.payload_descriptors[0];
        assert_eq!(decoded.container, TEST_CONTAINER);
        assert!(decoded.block_samples.is_none());
        assert!(decoded.codec_metadata.is_none());
    }

    #[test]
    fn invalid_descriptor_geometry_is_rejected_by_encoder() {
        let mut descriptor =
            PayloadDescriptorInput::ecdc(48_000, 2, &ecdc_codec_metadata()).unwrap();
        descriptor.container = "TESTCODEC".to_string();
        // 1000 + 64000 = 65000 > 64960 block.
        descriptor.output_offset_samples = Some(1_000);
        let input = RecordStreamInput {
            payload_descriptors: vec![descriptor],
            tracks: vec![TrackInput {
                title: "One".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: vec![0u8; 8],
        }];
        assert!(encode_record_stream(&input, &entries).is_err());
    }
}
