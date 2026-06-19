//! Reusable verbose inspection for Bitneedle PNG records.
//!
//! BRD1 and BRS1 remain compact binary wire formats. This library renders the
//! decoded typed structures as human-readable text (and optionally pretty JSON)
//! for diagnostics. Presentation JSON is never treated as canonical wire data.

use std::fmt::Write as _;
use std::io::Cursor;

use anyhow::{bail, Context, Result};
use encodec_rs::binary::{read_chunk_payload, read_ecdc_header};
use encodec_rs::format::{segment_starts, EcdcMetadata};
use record_core::{
    inspect_record_stream, parse_record_stream, payload_descriptor_count_from_metadata,
    validate_payload_entries_metadata, validate_track_listing_metadata, RecordStreamMetadata,
    ResolvedPayloadEntry, CONTAINER_ECDC, CONTAINER_EXTENSION, CONTAINER_MOSS_NANO,
    CONTAINER_RAW, RECORD_STREAM_HEADER_LENGTH, RECORD_STREAM_MAGIC,
};
use record_descriptor::{RecordDescriptor, SignedReleaseReference};

#[derive(Debug, Clone, Default)]
pub struct InspectionOptions<'a> {
    /// Optional display name for the PNG.
    pub png_name: Option<&'a str>,

    /// Optional EnCodec bundle metadata used only for ECDC packet-layout
    /// diagnostics.
    pub bundle_metadata: Option<&'a encodec_rs::metadata::OnnxFrameBundleMetadata>,

    /// Optional external release-manifest bytes. BRD1 contains only a binary
    /// signed-release reference, so the complete manifest must be supplied
    /// separately (or loaded from BSC/registry by a caller).
    pub manifest: Option<ExternalManifest<'a>>,

    /// Maximum number of transport chunks printed in detail.
    pub max_chunks: usize,

    /// Maximum prefix bytes printed for each binary object.
    pub max_hex_bytes: usize,
}

impl<'a> InspectionOptions<'a> {
    pub fn verbose_defaults() -> Self {
        Self {
            png_name: None,
            bundle_metadata: None,
            manifest: None,
            max_chunks: 12,
            max_hex_bytes: 256,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExternalManifest<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// Decode and inspect a complete Bitneedle PNG.
pub fn inspect_record_png(
    png: &[u8],
    options: &InspectionOptions<'_>,
) -> Result<String> {
    let decoded = record_decode::decode_record_png(png)
        .context("record_decode::decode_record_png failed")?;

    let mut out = String::new();

    section(&mut out, "FILE");
    writeln!(
        out,
        "  name:  {}",
        options.png_name.unwrap_or("<memory>")
    )?;
    writeln!(out, "  bytes: {}", png.len())?;

    if let Some((width, height, bit_depth, color_type)) = png_ihdr(png) {
        writeln!(
            out,
            "  PNG:   {width}x{height}, bit_depth={bit_depth}, color_type={color_type}"
        )?;
    } else {
        writeln!(out, "  PNG:   <could not read IHDR>")?;
    }

    writeln!(
        out,
        "  decoded record profile: {}",
        decoded.record_profile
    )?;

    report_descriptor(
        &mut out,
        &decoded.descriptor,
        options.manifest,
        options.max_hex_bytes,
    )?;

    report_stream(
        &mut out,
        &decoded.chunk_stream.bytes,
        decoded.chunk_stream.pixel_count,
        options,
    )?;

    Ok(out)
}

/// Render a decoded BRD1 descriptor.
pub fn report_descriptor(
    out: &mut String,
    descriptor: &RecordDescriptor,
    manifest: Option<ExternalManifest<'_>>,
    max_hex_bytes: usize,
) -> Result<()> {
    section(out, "BRD1 RECORD DESCRIPTOR");

    writeln!(out, "  version:              {}", descriptor.version)?;
    writeln!(
        out,
        "  checksum protected:   {}",
        descriptor.checksum_protected
    )?;
    writeln!(out, "  b_value:              {}", descriptor.b_value())?;
    writeln!(
        out,
        "  stream byte length:   {:?}",
        descriptor.stream_byte_length
    )?;
    writeln!(
        out,
        "  record profile:       {}",
        descriptor.record_profile
    )?;
    writeln!(
        out,
        "  payload encoding:     {}",
        descriptor.payload_encoding
    )?;
    writeln!(out, "  title:                {:?}", descriptor.title)?;
    writeln!(out, "  artist:               {:?}", descriptor.artist)?;
    writeln!(
        out,
        "  release ID:           {:?}",
        descriptor.release_id
    )?;
    writeln!(
        out,
        "  catalog number:       {:?}",
        descriptor.catalog_number
    )?;
    writeln!(out, "  label:                {:?}", descriptor.label)?;
    writeln!(
        out,
        "  artwork credit:       {:?}",
        descriptor.artwork_credit
    )?;
    writeln!(
        out,
        "  canonical URL:        {:?}",
        descriptor.canonical_url
    )?;
    writeln!(
        out,
        "  created at:           {:?}",
        descriptor.created_at
    )?;

    match descriptor.bsc_pointer.as_deref() {
        Some(pointer) => {
            writeln!(out, "  BSC pointer bytes:     {}", pointer.len())?;
            writeln!(
                out,
                "{}",
                indent(&hex_prefix(pointer, max_hex_bytes), 4)
            )?;
        }
        None => writeln!(out, "  BSC pointer:           absent")?,
    }

    report_signed_release_reference(
        out,
        descriptor.signed_release_reference.as_ref(),
        max_hex_bytes,
    )?;

    if let Some(manifest) = manifest {
        report_external_manifest(
            out,
            manifest,
            descriptor.signed_release_reference.as_ref(),
            max_hex_bytes,
        )?;
    } else if descriptor.signed_release_reference.is_some() {
        writeln!(
            out,
            "  manifest body:         not embedded; supply an external manifest or resolve BSC/registry data"
        )?;
    }

    Ok(())
}

pub fn report_signed_release_reference(
    out: &mut String,
    reference: Option<&SignedReleaseReference>,
    max_hex_bytes: usize,
) -> Result<()> {
    section(out, "SIGNED RELEASE REFERENCE");

    let Some(reference) = reference else {
        writeln!(out, "  absent")?;
        return Ok(());
    };

    reference.validate()?;

    writeln!(out, "  envelope version:        {}", reference.version)?;
    writeln!(
        out,
        "  manifest hash algorithm: {}",
        reference.manifest_hash_algorithm
    )?;
    writeln!(
        out,
        "  manifest hash bytes:     {}",
        reference.manifest_hash.len()
    )?;
    writeln!(
        out,
        "  manifest hash (hex):     {}",
        hex::encode(&reference.manifest_hash)
    )?;
    writeln!(
        out,
        "  signature algorithm:     {}",
        reference.signature_algorithm
    )?;

    match printable_utf8(&reference.key_id) {
        Some(text) => writeln!(out, "  key ID (UTF-8):           {text}")?,
        None => writeln!(
            out,
            "  key ID (hex):             {}",
            hex::encode(&reference.key_id)
        )?,
    }

    writeln!(
        out,
        "  key ID bytes:            {}",
        reference.key_id.len()
    )?;
    writeln!(
        out,
        "  signature bytes:         {}",
        reference.signature.len()
    )?;
    writeln!(out, "  signature prefix:")?;
    writeln!(
        out,
        "{}",
        indent(
            &hex_prefix(&reference.signature, max_hex_bytes.min(64)),
            4
        )
    )?;

    Ok(())
}

/// Render an external manifest without declaring any canonical manifest format.
///
/// If the bytes are UTF-8 JSON, they are pretty-printed solely for inspection.
/// Otherwise printable UTF-8 or a hexadecimal prefix is shown. Hash comparison
/// is deliberately left unresolved until algorithm-code semantics are fixed.
pub fn report_external_manifest(
    out: &mut String,
    manifest: ExternalManifest<'_>,
    reference: Option<&SignedReleaseReference>,
    max_hex_bytes: usize,
) -> Result<()> {
    section(out, "EXTERNAL RELEASE MANIFEST");

    writeln!(out, "  source:                 {}", manifest.name)?;
    writeln!(out, "  bytes:                  {}", manifest.bytes.len())?;

    if let Some(reference) = reference {
        writeln!(
            out,
            "  expected hash algorithm: {}",
            reference.manifest_hash_algorithm
        )?;
        writeln!(
            out,
            "  expected hash (hex):     {}",
            hex::encode(&reference.manifest_hash)
        )?;
        writeln!(
            out,
            "  hash comparison:         not attempted; algorithm registry is intentionally unsettled"
        )?;
    } else {
        writeln!(out, "  expected hash:           unavailable")?;
    }

    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(manifest.bytes) {
        writeln!(out, "  display format:          JSON (tooling view only)")?;
        let pretty = serde_json::to_string_pretty(&value)
            .context("failed to pretty-print manifest JSON")?;
        writeln!(out, "{}", indent(&pretty, 4))?;
    } else if let Some(text) = printable_utf8(manifest.bytes) {
        writeln!(out, "  display format:          UTF-8 text")?;
        writeln!(out, "{}", indent(text, 4))?;
    } else {
        writeln!(out, "  display format:          binary")?;
        writeln!(
            out,
            "{}",
            indent(&hex_prefix(manifest.bytes, max_hex_bytes), 4)
        )?;
    }

    Ok(())
}

/// Render a BRS1 stream and its logical payload entries.
pub fn report_stream(
    out: &mut String,
    stream: &[u8],
    extracted_groove_pixels: usize,
    options: &InspectionOptions<'_>,
) -> Result<()> {
    section(out, "BRS1 RECORD STREAM");

    writeln!(
        out,
        "  extracted groove pixels: {}",
        extracted_groove_pixels
    )?;
    writeln!(out, "  stream bytes:             {}", stream.len())?;
    writeln!(out, "  stream magic:             {:?}", ascii_magic(stream))?;
    writeln!(
        out,
        "  BRS1 magic valid:         {}",
        stream.get(..4) == Some(RECORD_STREAM_MAGIC.as_slice())
    )?;

    let header_end = record_core::record_stream_header_end(stream)?;
    writeln!(out, "  BRS1 header end:           {header_end}")?;
    writeln!(
        out,
        "  binary metadata bytes:    {}",
        header_end.saturating_sub(RECORD_STREAM_HEADER_LENGTH)
    )?;
    writeln!(
        out,
        "  chunk-section bytes:      {}",
        stream.len().saturating_sub(header_end)
    )?;
    writeln!(out, "  stream prefix:")?;
    writeln!(
        out,
        "{}",
        indent(&hex_prefix(stream, options.max_hex_bytes), 4)
    )?;

    let parsed = parse_record_stream(stream)?;
    let inspection = inspect_record_stream(&parsed)?;

    section(out, "BRS1 TYPED METADATA");

    // JSON here is a presentation format generated from the typed structure.
    // It is not read from the BRS1 wire and does not determine canonical bytes.
    let pretty = serde_json::to_string_pretty(&inspection)
        .context("failed to render typed BRS1 inspection")?;
    writeln!(out, "{}", indent(&pretty, 2))?;

    report_metadata_summary(out, &parsed.metadata)?;
    report_chunks(out, &parsed, options.max_chunks)?;
    report_entries(out, &parsed, options)?;

    Ok(())
}

fn report_metadata_summary(
    out: &mut String,
    metadata: &RecordStreamMetadata,
) -> Result<()> {
    section(out, "BRS1 CANONICAL METADATA CHECK");

    let descriptor_count = payload_descriptor_count_from_metadata(metadata)?;
    validate_track_listing_metadata(metadata)?;

    writeln!(out, "  metadata version:          {}", metadata.version)?;
    writeln!(out, "  encrypted:                 {}", metadata.encrypted)?;
    writeln!(out, "  payload descriptors:       {descriptor_count}")?;
    writeln!(
        out,
        "  payload entries:           {}",
        metadata.payload_entries.len()
    )?;
    writeln!(out, "  tracks:                    {}", metadata.tracks.len())?;

    for (index, descriptor) in metadata.payload_descriptors.iter().enumerate() {
        writeln!(
            out,
            "  descriptor[{index}]: container={} codec={:?} sample_rate={:?} channels={:?}",
            descriptor.container,
            descriptor.codec,
            descriptor.sample_rate,
            descriptor.channels
        )?;
    }

    section(out, "BRS1 TRACK LISTING");

    for (index, track) in metadata.tracks.iter().enumerate() {
        writeln!(
            out,
            "  track[{}]: title={:?} payload_entry_index={}",
            index + 1,
            track.title,
            track.payload_entry_index
        )?;
    }

    Ok(())
}

fn report_chunks(
    out: &mut String,
    parsed: &record_core::RecordStream,
    max_chunks: usize,
) -> Result<()> {
    section(out, "BRS1 TRANSPORT CHUNKS");

    writeln!(out, "  chunk count: {}", parsed.chunks.len())?;

    for (index, chunk) in parsed.chunks.iter().enumerate().take(max_chunks) {
        writeln!(
            out,
            "  chunk[{index}]: stream_index={} chunk_count={} descriptor_index={} payload_bytes={} crc32={:08x} nonce={}",
            chunk.index,
            chunk.chunk_count,
            chunk.payload_descriptor_index,
            chunk.payload.len(),
            chunk.crc32,
            if chunk.nonce.is_some() { "present" } else { "absent" }
        )?;
    }

    if parsed.chunks.len() > max_chunks {
        writeln!(
            out,
            "  ... {} more chunks",
            parsed.chunks.len() - max_chunks
        )?;
    }

    Ok(())
}

fn report_entries(
    out: &mut String,
    parsed: &record_core::RecordStream,
    options: &InspectionOptions<'_>,
) -> Result<()> {
    section(out, "BRS1 PAYLOAD ENTRIES");

    let payload = record_core::chunk_stream_payload_bytes(parsed);
    let entries = validate_payload_entries_metadata(
        &parsed.metadata,
        Some(payload.len()),
    )?;

    writeln!(
        out,
        "  reconstructed payload bytes: {}",
        payload.len()
    )?;
    writeln!(out, "  logical payload entries:     {}", entries.len())?;

    for entry in &entries {
        let bytes = payload_entry_bytes(&payload, entry)?;

        let descriptor = parsed
            .metadata
            .payload_descriptors
            .get(entry.payload_descriptor_index as usize)
            .context("payload descriptor index is out of range")?;

        writeln!(
            out,
            "  entry[{}]: offset={} byte_length={} descriptor_index={} container={} magic={:?}",
            entry.index,
            entry.byte_offset,
            entry.byte_length,
            entry.payload_descriptor_index,
            descriptor.container,
            ascii_magic(bytes)
        )?;

        section(
            out,
            &format!(
                "PAYLOAD ENTRY {} / {}",
                entry.index + 1,
                entries.len()
            ),
        );

        writeln!(out, "  index:                    {}", entry.index)?;
        writeln!(
            out,
            "  byte offset:              {}",
            entry.byte_offset
        )?;
        writeln!(
            out,
            "  byte length:              {}",
            entry.byte_length
        )?;
        writeln!(
            out,
            "  payload descriptor index: {}",
            entry.payload_descriptor_index
        )?;
        writeln!(
            out,
            "  container:                {}",
            descriptor.container
        )?;
        writeln!(out, "  codec:                    {:?}", descriptor.codec)?;
        writeln!(
            out,
            "  sample rate:              {:?}",
            descriptor.sample_rate
        )?;
        writeln!(
            out,
            "  channels:                 {:?}",
            descriptor.channels
        )?;
        writeln!(out, "  magic:                    {:?}", ascii_magic(bytes))?;

        if container_is_ecdc(&descriptor.container) {
            analyse_ecdc_entry(out, bytes, options.bundle_metadata)?;
        } else {
            writeln!(out, "  payload prefix:")?;
            writeln!(
                out,
                "{}",
                indent(&hex_prefix(bytes, options.max_hex_bytes.min(128)), 4)
            )?;
        }
    }

    Ok(())
}

fn payload_entry_bytes<'a>(
    payload: &'a [u8],
    entry: &ResolvedPayloadEntry,
) -> Result<&'a [u8]> {
    let end = entry
        .byte_offset
        .checked_add(entry.byte_length)
        .context("payload entry range overflow")?;

    payload
        .get(entry.byte_offset..end)
        .context("payload entry range exceeds reconstructed payload")
}

fn container_is_ecdc(container: &str) -> bool {
    container.eq_ignore_ascii_case(record_core::PAYLOAD_CONTAINER_ECDC)
        || container == CONTAINER_ECDC.to_string()
}

fn analyse_ecdc_entry(
    out: &mut String,
    entry: &[u8],
    bundle_metadata: Option<&encodec_rs::metadata::OnnxFrameBundleMetadata>,
) -> Result<()> {
    writeln!(out, "  -- ECDC payload --")?;
    writeln!(out, "  bytes:  {}", entry.len())?;
    writeln!(out, "  magic:  {:?}", ascii_magic(entry))?;
    writeln!(out, "  prefix:")?;
    writeln!(out, "{}", indent(&hex_prefix(entry, 96), 4))?;

    let mut reader = Cursor::new(entry);
    let metadata: EcdcMetadata = match read_ecdc_header(&mut reader) {
        Ok(metadata) => metadata,
        Err(err) => {
            writeln!(out, "  !! read_ecdc_header FAILED: {err:#}")?;
            return Ok(());
        }
    };

    let header_end = reader.position() as usize;

    writeln!(
        out,
        "  ECDC header parsed: {} bytes, {} bytes remain for packets",
        header_end,
        entry.len().saturating_sub(header_end)
    )?;
    writeln!(out, "  model name:              {}", metadata.model_name)?;
    writeln!(out, "  audio length:            {}", metadata.audio_length)?;
    writeln!(out, "  num codebooks:           {}", metadata.num_codebooks)?;
    writeln!(out, "  use LM:                  {}", metadata.use_lm)?;
    writeln!(
        out,
        "  bitstream version:       {}",
        metadata.bitstream_version
    )?;
    writeln!(out, "  LM hash:                 {:?}", metadata.lm_hash)?;
    writeln!(
        out,
        "  chunk samples:           {:?}",
        metadata.chunk_samples
    )?;
    writeln!(
        out,
        "  chunk stride:            {:?}",
        metadata.chunk_stride
    )?;
    writeln!(
        out,
        "  LM frame length:         {:?}",
        metadata.lm_frame_length
    )?;

    if !metadata.extra.is_empty() {
        writeln!(out, "  extra:                   {:?}", metadata.extra)?;
    }

    let mut raw_chunks = 0usize;
    let mut chunk_error: Option<String> = None;

    loop {
        if reader.position() as usize >= entry.len() {
            break;
        }

        match read_chunk_payload(&mut reader, true) {
            Ok(payload) => {
                if raw_chunks < 12 {
                    writeln!(
                        out,
                        "  ECDC packet[{raw_chunks}]: payload_bytes={}",
                        payload.len()
                    )?;
                }
                raw_chunks += 1;
            }
            Err(err) => {
                chunk_error = Some(format!("{err:#}"));
                break;
            }
        }
    }

    if raw_chunks > 12 {
        writeln!(out, "  ... {} more ECDC packets", raw_chunks - 12)?;
    }

    writeln!(out, "  ECDC packet bodies present: {raw_chunks}")?;

    if let Some(err) = chunk_error {
        writeln!(out, "  !! ECDC packet read stopped early: {err}")?;
    }

    report_implied_chunks(out, &metadata, bundle_metadata, raw_chunks)?;

    Ok(())
}

fn report_implied_chunks(
    out: &mut String,
    metadata: &EcdcMetadata,
    bundle_metadata: Option<&encodec_rs::metadata::OnnxFrameBundleMetadata>,
    raw_chunks: usize,
) -> Result<()> {
    writeln!(out, "  -- implied ECDC packet count --")?;

    if let Some(stride) = metadata.chunk_stride {
        let implied = segment_starts(metadata.audio_length, stride).len();
        writeln!(
            out,
            "  metadata chunk_stride={stride}: implies {implied} packets"
        )?;
        verdict(out, raw_chunks, implied)?;
        return Ok(());
    }

    if let Some(bundle) = bundle_metadata {
        match encodec_rs::format::ecdc_chunk_layout_for_chunk_count(
            bundle,
            metadata,
            raw_chunks,
        ) {
            Ok(layout) => {
                let implied =
                    segment_starts(metadata.audio_length, layout.stride).len();
                writeln!(
                    out,
                    "  bundle stride={}: implies {implied} packets",
                    layout.stride
                )?;
                verdict(out, raw_chunks, implied)?;
            }
            Err(err) => {
                writeln!(
                    out,
                    "  ecdc_chunk_layout_for_chunk_count: {err:#}"
                )?;
            }
        }

        return Ok(());
    }

    writeln!(
        out,
        "  no chunk_stride in ECDC metadata and no bundle metadata was provided"
    )?;

    Ok(())
}

fn verdict(out: &mut String, raw_chunks: usize, implied: usize) -> Result<()> {
    if raw_chunks == implied {
        writeln!(
            out,
            "  => OK: {raw_chunks} packet bodies match {implied} implied"
        )?;
    } else {
        writeln!(
            out,
            "  => MISMATCH: {raw_chunks} packet bodies are present, but metadata implies {implied}"
        )?;
    }

    Ok(())
}

pub fn load_bundle_metadata(
    path: impl AsRef<std::path::Path>,
) -> Result<encodec_rs::metadata::OnnxFrameBundleMetadata> {
    let path = path.as_ref();
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read bundle JSON {}", path.display()))?;

    serde_json::from_str(&json)
        .context("failed to deserialize OnnxFrameBundleMetadata")
}

fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n=== {title} ===");
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);

    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn printable_utf8(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;

    if text.chars().any(|character| {
        character.is_control() && !matches!(character, '\n' | '\r' | '\t')
    }) {
        return None;
    }

    Some(text)
}

fn ascii_magic(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|byte| {
            if (0x20..=0x7e).contains(byte) {
                *byte as char
            } else {
                '.'
            }
        })
        .collect()
}

fn hex_prefix(bytes: &[u8], max: usize) -> String {
    let mut out = String::new();
    let clipped = bytes.len().min(max);

    for offset in (0..clipped).step_by(16) {
        let end = (offset + 16).min(clipped);
        let chunk = &bytes[offset..end];

        let hex = chunk
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");

        let ascii = chunk
            .iter()
            .map(|byte| {
                if (0x20..=0x7e).contains(byte) {
                    *byte as char
                } else {
                    '.'
                }
            })
            .collect::<String>();

        out.push_str(&format!("{offset:08x}: {hex:<47}  {ascii}\n"));
    }

    if bytes.len() > max {
        out.push_str(&format!("... truncated {} bytes\n", bytes.len() - max));
    }

    out
}

fn png_ihdr(png: &[u8]) -> Option<(u32, u32, u8, u8)> {
    if png.len() < 33
        || &png[..8] != b"\x89PNG\r\n\x1a\n"
        || &png[12..16] != b"IHDR"
    {
        return None;
    }

    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);

    Some((width, height, png[24], png[25]))
}

#[allow(dead_code)]
fn container_code_name(code: u8) -> &'static str {
    match code {
        CONTAINER_RAW => "RAW",
        CONTAINER_ECDC => "ECDC",
        CONTAINER_MOSS_NANO => "MOSSNANO",
        CONTAINER_EXTENSION => "EXTENSION",
        _ => "UNKNOWN",
    }
}
