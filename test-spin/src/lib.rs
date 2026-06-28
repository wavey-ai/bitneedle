//! Reusable verbose inspection for Bitneedle PNG records.
//!
//! BRD1 and BRS1 remain compact binary wire formats. This library renders the
//! decoded typed structures as labelled human-readable text for diagnostics.
//! Canonical BRD1 and BRS1 data remains compact binary.

use anyhow::{bail, Context, Result};
use record_core::{
    gap, parse_record_stream, payload_descriptor_count_from_metadata,
    validate_payload_entries_metadata, validate_track_listing_metadata, RecordStreamMetadata,
    ResolvedPayloadEntry, CONTAINER_ECDC, CONTAINER_EXTENSION, CONTAINER_MOSS_NANO,
    PAYLOAD_CONTAINER_ECDC, PAYLOAD_CONTAINER_GAP, RECORD_STREAM_HEADER_LENGTH,
    RECORD_STREAM_MAGIC,
};
use record_descriptor::{RecordDescriptor, SignedReleaseReference};
use std::fmt::Write as _;

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

    /// Print every payload entry instead of the default first/last compact view.
    pub verbose_payload_entries: bool,
}

impl<'a> InspectionOptions<'a> {
    pub fn verbose_defaults() -> Self {
        Self {
            png_name: None,
            bundle_metadata: None,
            manifest: None,
            max_chunks: 12,
            max_hex_bytes: 256,
            verbose_payload_entries: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExternalManifest<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// Decode and inspect a complete Bitneedle PNG.
pub fn inspect_record_png(png: &[u8], options: &InspectionOptions<'_>) -> Result<String> {
    let decoded =
        record_decode::decode_record_png(png).context("record_decode::decode_record_png failed")?;

    let mut out = String::new();

    section(&mut out, "FILE");
    writeln!(out, "  name:  {}", options.png_name.unwrap_or("<memory>"))?;
    writeln!(out, "  bytes: {}", png.len())?;

    if let Some((width, height, bit_depth, color_type)) = png_ihdr(png) {
        writeln!(
            out,
            "  PNG:   {width}x{height}, bit_depth={bit_depth}, color_type={color_type}"
        )?;
    } else {
        writeln!(out, "  PNG:   <could not read IHDR>")?;
    }

    writeln!(out, "  decoded record profile: {}", decoded.record_profile)?;

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
        &decoded.record_profile,
        options,
    )?;
    report_signing_state(
        &mut out,
        &decoded.descriptor,
        &decoded.chunk_stream.bytes,
        options.manifest,
    )?;
    report_final_summary(
        &mut out,
        png,
        &decoded.descriptor,
        &decoded.chunk_stream.bytes,
        &decoded.record_profile,
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
        "  stream byte length:   {}",
        descriptor.stream_byte_length.to_string()
    )?;
    writeln!(out, "  record profile:       {}", descriptor.record_profile)?;
    writeln!(
        out,
        "  payload encoding:     {}",
        descriptor.payload_encoding
    )?;
    writeln!(out, "  title:                {}", format_optional_text(descriptor.title.as_deref()))?;
    writeln!(out, "  artist:               {}", format_optional_text(descriptor.artist.as_deref()))?;
    writeln!(
        out,
        "  release ID:           {}",
        descriptor
            .release_id
            .map(record_descriptor::release_id_to_text)
            .unwrap_or_else(|| "absent".to_owned())
    )?;
    writeln!(
        out,
        "  catalog number:       {}",
        format_optional_text(descriptor.catalog_number.as_deref())
    )?;
    writeln!(out, "  label:                {}", format_optional_text(descriptor.label.as_deref()))?;
    writeln!(
        out,
        "  artwork credit:       {}",
        format_optional_text(descriptor.artwork_credit.as_deref())
    )?;
    writeln!(
        out,
        "  canonical URL:        {}",
        format_optional_text(descriptor.canonical_url.as_deref())
    )?;
    writeln!(
        out,
        "  YL catalogue code:    {}",
        descriptor
            .canonical_url
            .as_deref()
            .and_then(yl_catalogue_code_from_url)
            .unwrap_or_else(|| "absent".to_owned())
    )?;
    writeln!(
        out,
        "  created at:           {}",
        format_optional_number(descriptor.created_at)
    )?;

    match descriptor.bsc_pointer.as_deref() {
        Some(pointer) => {
            writeln!(out, "  BSC pointer bytes:     {}", pointer.len())?;
            writeln!(out, "{}", indent(&hex_prefix(pointer, max_hex_bytes), 4))?;
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
        "  release commitment bytes: {}",
        reference.release_commitment_sha256.len()
    )?;
    writeln!(
        out,
        "  release commitment (hex): {}",
        hex::encode(reference.release_commitment_sha256)
    )?;

    match printable_utf8(&reference.key_id) {
        Some(text) => writeln!(out, "  key ID (UTF-8):           {text}")?,
        None => writeln!(
            out,
            "  key ID (hex):             {}",
            hex::encode(&reference.key_id)
        )?,
    }

    writeln!(out, "  key ID bytes:            {}", reference.key_id.len())?;
    writeln!(
        out,
        "  signature bytes:         {}",
        reference.signature.len()
    )?;
    writeln!(out, "  signature prefix:")?;
    writeln!(
        out,
        "{}",
        indent(&hex_prefix(&reference.signature, max_hex_bytes.min(64)), 4)
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
            "  expected release commitment (hex): {}",
            hex::encode(reference.release_commitment_sha256)
        )?;
        writeln!(
            out,
            "  hash comparison:         not attempted; algorithm registry is intentionally unsettled"
        )?;
    } else {
        writeln!(out, "  expected hash:           unavailable")?;
    }

    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(manifest.bytes) {
        writeln!(out, "  display format:          JSON")?;
        report_json_structure(out, &value)?;
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
    record_profile: &str,
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

    report_stream_summary(out, &parsed)?;
    report_metadata_summary(out, &parsed.metadata)?;
    report_chunks(out, &parsed, options.max_chunks)?;
    report_actual_programme_layout(
        out,
        &parsed,
        record_profile,
        options.verbose_payload_entries,
    )?;
    report_entries(out, &parsed, options)?;
    report_spec_consistency(out, &parsed, record_profile)?;

    Ok(())
}

fn report_stream_summary(out: &mut String, parsed: &record_core::RecordStream) -> Result<()> {
    section(out, "BRS1 SUMMARY");

    let musical_revolutions = parsed
        .metadata
        .tracks
        .iter()
        .map(|track| track.revolution_count)
        .sum::<usize>();

    // Track-gap entries are reported purely from the explicit `track_gaps`
    // metadata — never by sniffing a payload container, classifying small
    // ECDC entries as gaps, or inferring gaps from entries left uncovered by
    // a track. A gap's bytes are an ordinary payload entry (ECDC in
    // practice); only this metadata says what it is.
    let track_gap_entries = parsed
        .metadata
        .track_gaps
        .iter()
        .map(|gap| gap.revolution_count as usize)
        .sum::<usize>();

    let total_entries = parsed.metadata.payload_entries.len();

    writeln!(out, "  report mode:               compact-v3")?;
    writeln!(
        out,
        "  tracks:                    {}",
        parsed.metadata.tracks.len()
    )?;
    writeln!(
        out,
        "  TrackGap ranges:           {}",
        parsed.metadata.track_gaps.len()
    )?;
    writeln!(out, "  musical timeline entries:  {musical_revolutions}")?;
    writeln!(out, "  TrackGap timeline entries: {track_gap_entries}")?;
    writeln!(out, "  total timeline entries:    {total_entries}")?;
    writeln!(out, "  transport chunks:          {}", parsed.chunks.len())?;

    if musical_revolutions + track_gap_entries != total_entries {
        bail!(
            "musical timeline entries ({musical_revolutions}) + TrackGap timeline entries \
             ({track_gap_entries}) != total timeline entries ({total_entries}); every payload \
             entry must belong to exactly one track or track gap"
        );
    }

    Ok(())
}

fn report_metadata_summary(out: &mut String, metadata: &RecordStreamMetadata) -> Result<()> {
    section(out, "BRS1 HEADER METADATA");

    let descriptor_count = payload_descriptor_count_from_metadata(metadata)?;

    writeln!(out, "  metadata version:          {}", metadata.version)?;
    writeln!(out, "  encrypted:                 {}", metadata.encrypted)?;
    writeln!(out, "  payload descriptors:       {descriptor_count}")?;
    writeln!(
        out,
        "  payload entries:           {}",
        metadata.payload_entries.len()
    )?;
    writeln!(
        out,
        "  tracks:                    {}",
        metadata.tracks.len()
    )?;

    section(out, "BRS1 PAYLOAD DESCRIPTORS");

    for (index, descriptor) in metadata.payload_descriptors.iter().enumerate() {
        writeln!(out, "  descriptor[{index}]")?;
        writeln!(out, "    container:              {}", descriptor.container)?;
        writeln!(
            out,
            "    codec:                  {}",
            descriptor.codec.as_deref().unwrap_or("<absent>")
        )?;
        writeln!(
            out,
            "    sample rate:            {}",
            descriptor
                .sample_rate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<absent>".to_owned())
        )?;
        writeln!(
            out,
            "    channels:               {}",
            descriptor
                .channels
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<absent>".to_owned())
        )?;
        writeln!(
            out,
            "    block samples:          {}",
            descriptor
                .block_samples
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<absent>".to_owned())
        )?;
        writeln!(
            out,
            "    output offset samples:  {}",
            descriptor
                .output_offset_samples
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<absent>".to_owned())
        )?;
        writeln!(
            out,
            "    output samples:         {}",
            descriptor
                .output_samples
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<absent>".to_owned())
        )?;

        match descriptor.codec_metadata.as_deref() {
            Some(bytes) => {
                writeln!(out, "    codec metadata bytes:   {}", bytes.len())?;
                match std::str::from_utf8(bytes) {
                    Ok(text) => {
                        writeln!(out, "    codec metadata UTF-8:   yes")?;
                        match serde_json::from_str::<serde_json::Value>(text) {
                            Ok(value) => {
                                writeln!(out, "    codec metadata JSON:    yes")?;
                                writeln!(
                                    out,
                                    "    codec metadata value:   {}",
                                    serde_json::to_string(&value)?
                                )?;
                            }
                            Err(error) => {
                                writeln!(out, "    codec metadata JSON:    no")?;
                                writeln!(out, "    codec metadata error:   {error}")?;
                                writeln!(out, "{}", indent(&hex_prefix(bytes, 384), 6))?;
                            }
                        }
                    }
                    Err(error) => {
                        writeln!(out, "    codec metadata UTF-8:   no")?;
                        writeln!(out, "    codec metadata error:   {error}")?;
                        writeln!(out, "{}", indent(&hex_prefix(bytes, 384), 6))?;
                    }
                }
            }
            None => {
                writeln!(out, "    codec metadata:         absent")?;
            }
        }
    }

    section(out, "BRS1 PAYLOAD ENTRY TABLE");

    let display_indices = track_boundary_entry_indices(metadata);
    let mut byte_offset = 0usize;
    let mut previous_printed_index = None;

    for (index, entry) in metadata.payload_entries.iter().enumerate() {
        if display_indices.binary_search(&index).is_ok() {
            if let Some(previous_index) = previous_printed_index {
                let omitted = index.saturating_sub(previous_index + 1);
                if omitted > 0 {
                    writeln!(out, "  ... {omitted} intermediate payload entries omitted")?;
                }
            }

            writeln!(
                out,
                "  entry[{index}]: byte_offset={} byte_length={} descriptor_index={}",
                byte_offset, entry.byte_length, entry.payload_descriptor_index
            )?;
            previous_printed_index = Some(index);
        }

        byte_offset = byte_offset
            .checked_add(entry.byte_length)
            .context("payload entry byte offset overflow while reporting header table")?;
    }

    section(out, "BRS1 TRACK TABLE");

    for (index, track) in metadata.tracks.iter().enumerate() {
        writeln!(
            out,
            "  track[{index}]: title={:?} first_revolution_index={} revolution_count={}",
            track.title, track.first_revolution_index, track.revolution_count
        )?;
    }

    Ok(())
}

fn track_boundary_entry_indices(metadata: &RecordStreamMetadata) -> Vec<usize> {
    let mut indices = Vec::new();

    for track in &metadata.tracks {
        if track.revolution_count == 0 {
            continue;
        }

        let first = track.first_revolution_index;
        let last = first.saturating_add(track.revolution_count.saturating_sub(1));

        if first < metadata.payload_entries.len() {
            indices.push(first);
        }
        if last < metadata.payload_entries.len() && last != first {
            indices.push(last);
        }
    }

    indices.sort_unstable();
    indices.dedup();
    indices
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
            "  chunk[{index}]: payload_bytes={} crc32={:08x} nonce={}",
            chunk.payload.len(),
            chunk.crc32,
            if chunk.nonce.is_some() {
                "present"
            } else {
                "absent"
            }
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

#[derive(Debug, Clone)]
struct SpecCheck {
    passed: bool,
    label: String,
    detail: String,
}

impl SpecCheck {
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            label: label.into(),
            detail: detail.into(),
        }
    }

    fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: false,
            label: label.into(),
            detail: detail.into(),
        }
    }
}

fn report_actual_programme_layout(
    out: &mut String,
    parsed: &record_core::RecordStream,
    record_profile: &str,
    _verbose_payload_entries: bool,
) -> Result<()> {
    section(out, "DERIVED PROGRAMME LAYOUT");

    let payload = record_core::record_stream_payload_bytes(parsed);
    let entries = validate_payload_entries_metadata(&parsed.metadata, Some(payload.len()))?;

    let mut track_by_entry: Vec<Option<(usize, &str)>> =
        vec![None; parsed.metadata.payload_entries.len()];

    for (track_index, track) in parsed.metadata.tracks.iter().enumerate() {
        let end = track
            .first_revolution_index
            .checked_add(track.revolution_count)
            .context("track revolution range overflow while reporting layout")?;

        for entry_index in track.first_revolution_index..end {
            if let Some(slot) = track_by_entry.get_mut(entry_index) {
                *slot = Some((track_index + 1, track.title.as_str()));
            }
        }
    }

    let mut chunk_ranges = Vec::with_capacity(parsed.chunks.len());
    let mut chunk_offset = 0usize;
    for (chunk_index, chunk) in parsed.chunks.iter().enumerate() {
        let end = chunk_offset
            .checked_add(chunk.payload.len())
            .context("transport chunk byte range overflow")?;
        chunk_ranges.push((chunk_index, chunk_offset, end));
        chunk_offset = end;
    }

    let sample_rate = parsed
        .metadata
        .payload_descriptors
        .iter()
        .find_map(|descriptor| descriptor.sample_rate);

    let display_indices = track_boundary_entry_indices(&parsed.metadata);
    let mut sample_cursor = 0u64;
    let mut sample_cursor_known = true;
    let mut previous_printed_index = None;

    writeln!(
        out,
        "  note:                      derived from header tables and payload bodies"
    )?;
    writeln!(out, "  record profile:            {record_profile}")?;
    writeln!(out, "  logical payload entries:   {}", entries.len())?;
    writeln!(out, "  transport chunks:          {}", parsed.chunks.len())?;
    writeln!(out, "  stored payload bytes:      {}", payload.len())?;
    writeln!(
        out,
        "  sample rate:               {}",
        sample_rate
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<absent>".to_owned())
    )?;

    for entry in &entries {
        let descriptor = parsed
            .metadata
            .payload_descriptors
            .get(entry.payload_descriptor_index as usize)
            .context("payload descriptor index is out of range")?;

        let entry_end = entry
            .byte_offset
            .checked_add(entry.byte_length)
            .context("payload entry byte range overflow")?;

        let entry_bytes = payload
            .get(entry.byte_offset..entry_end)
            .context("payload entry exceeds stored payload")?;

        let ownership = track_by_entry[entry.index]
            .map(|(number, title)| format!("track {number} {:?}", title));

        let should_print = display_indices.binary_search(&entry.index).is_ok();

        let sample_count_result: Result<(u64, &'static str)> = if descriptor
            .container
            .eq_ignore_ascii_case(PAYLOAD_CONTAINER_GAP)
        {
            gap::decode_gap_header(entry_bytes)
                .map(|header| (header.sample_count, "GAP1 header"))
                .context("failed to read GAP1 sample count")
        } else if descriptor
            .container
            .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
        {
            record_core::ecdc::headerless_entry_sample_count(entry_bytes, descriptor)
                .map(|samples| (samples, "headerless ECDC entry"))
                .context("failed to read exact headerless ECDC sample count")
        } else {
            descriptor
                .output_samples
                .map(|samples| (u64::from(samples), "descriptor outputSamples"))
                .context("no exact sample-count rule for this payload entry")
        };

        let covering_chunks = chunk_ranges
            .iter()
            .filter_map(|(chunk_index, chunk_start, chunk_end)| {
                let overlap_start = entry.byte_offset.max(*chunk_start);
                let overlap_end = entry_end.min(*chunk_end);

                if overlap_start < overlap_end {
                    Some(format!(
                        "{chunk_index}[{}..{}]",
                        overlap_start - *chunk_start,
                        overlap_end - *chunk_start
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if should_print {
            if let Some(previous_index) = previous_printed_index {
                let omitted = entry.index.saturating_sub(previous_index + 1);
                if omitted > 0 {
                    writeln!(out, "  ... {omitted} intermediate payload entries omitted")?;
                }
            }

            writeln!(
                out,
                "  entry[{}] {}",
                entry.index,
                ownership.as_deref().unwrap_or("semantic gap")
            )?;
            writeln!(
                out,
                "    descriptor index:       {}",
                entry.payload_descriptor_index
            )?;
            writeln!(
                out,
                "    container / codec:      {} / {}",
                descriptor.container,
                descriptor.codec.as_deref().unwrap_or("<absent>")
            )?;
            writeln!(
                out,
                "    stored byte range:      {}..{} ({} bytes)",
                entry.byte_offset, entry_end, entry.byte_length
            )?;

            if descriptor
                .container
                .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
            {
                writeln!(out, "    payload prefix:         headerless codec body")?;
            } else {
                writeln!(
                    out,
                    "    payload magic:          {:?}",
                    ascii_magic(entry_bytes)
                )?;
            }

            writeln!(
                out,
                "    transport coverage:     {}",
                if covering_chunks.is_empty() {
                    "<none>".to_owned()
                } else {
                    covering_chunks.join(", ")
                }
            )?;
            previous_printed_index = Some(entry.index);
        }

        match sample_count_result {
            Ok((0, source))
                if descriptor
                    .container
                    .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC) =>
            {
                sample_cursor_known = false;
                if should_print {
                    writeln!(out, "    sample count:           invalid zero ({source})")?;
                    writeln!(
                        out,
                        "    programme samples:      unavailable from this entry onward"
                    )?;
                }
            }
            Ok((samples, source)) => {
                let start_sample = sample_cursor;
                let end_sample = start_sample
                    .checked_add(samples)
                    .context("programme sample range overflow")?;

                if should_print {
                    writeln!(out, "    sample count:           {samples} ({source})")?;
                    if sample_cursor_known {
                        writeln!(
                            out,
                            "    programme samples:      {start_sample}..{end_sample}"
                        )?;
                        if let Some(rate) = sample_rate.filter(|value| *value > 0) {
                            writeln!(
                                out,
                                "    programme time:         {:.6}..{:.6} s",
                                start_sample as f64 / rate as f64,
                                end_sample as f64 / rate as f64
                            )?;
                        }
                    } else {
                        writeln!(
                            out,
                            "    programme samples:      unknown because an earlier entry could not be measured"
                        )?;
                    }
                }

                sample_cursor = end_sample;
            }
            Err(error) => {
                sample_cursor_known = false;
                if should_print {
                    writeln!(out, "    sample count:           unavailable")?;
                    writeln!(out, "    sample-count error:     {error:#}")?;
                }
            }
        }
    }

    if sample_cursor_known {
        writeln!(out, "  total programme samples:   {sample_cursor}")?;
        if let Some(rate) = sample_rate.filter(|value| *value > 0) {
            writeln!(
                out,
                "  total programme duration:  {:.6} s",
                sample_cursor as f64 / rate as f64
            )?;
        }
    } else {
        writeln!(
            out,
            "  total programme samples:   unavailable because one or more entries could not be measured"
        )?;
    }

    Ok(())
}

fn collect_spec_checks(parsed: &record_core::RecordStream, record_profile: &str) -> Vec<SpecCheck> {
    let mut checks = Vec::new();

    checks.push(
        if parsed.metadata.version == record_core::RECORD_STREAM_METADATA_VERSION {
            SpecCheck::pass(
                "BRS1 metadata version",
                format!("version {}", parsed.metadata.version),
            )
        } else {
            SpecCheck::fail(
                "BRS1 metadata version",
                format!(
                    "expected {}, found {}",
                    record_core::RECORD_STREAM_METADATA_VERSION,
                    parsed.metadata.version
                ),
            )
        },
    );

    checks.push(
        match record_core::normalize_record_profile_name(record_profile) {
            Ok(profile) => SpecCheck::pass("Record profile", profile),
            Err(error) => SpecCheck::fail("Record profile", format!("{error:#}")),
        },
    );

    checks.push(match validate_track_listing_metadata(&parsed.metadata) {
        Ok(()) => SpecCheck::pass(
            "Musical track ranges",
            "all payload entries are covered exactly once by either Track or TrackGap",
        ),
        Err(error) => SpecCheck::fail("Musical track ranges", format!("{error:#}")),
    });

    let payload = record_core::record_stream_payload_bytes(parsed);
    let resolved = match validate_payload_entries_metadata(&parsed.metadata, Some(payload.len())) {
        Ok(entries) => {
            checks.push(SpecCheck::pass(
                "Payload entry byte coverage",
                format!(
                    "{} entries cover all {} stored payload bytes",
                    entries.len(),
                    payload.len()
                ),
            ));
            Some(entries)
        }
        Err(error) => {
            checks.push(SpecCheck::fail(
                "Payload entry byte coverage",
                format!("{error:#}"),
            ));
            None
        }
    };

    for (index, descriptor) in parsed.metadata.payload_descriptors.iter().enumerate() {
        checks.push(match record_core::validate_payload_descriptor(descriptor) {
            Ok(()) => SpecCheck::pass(
                format!("Descriptor {index} generic validation"),
                format!("container {}", descriptor.container),
            ),
            Err(error) => SpecCheck::fail(
                format!("Descriptor {index} generic validation"),
                format!("{error:#}"),
            ),
        });

        if descriptor
            .container
            .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
        {
            let mut problems = Vec::new();

            if !matches!(
                descriptor.codec.as_deref(),
                Some(codec) if codec.eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
            ) {
                problems.push("codec is absent or not ECDC".to_owned());
            }
            if !matches!(descriptor.sample_rate, Some(value) if value > 0) {
                problems.push("sampleRate is absent or zero".to_owned());
            }
            if !matches!(descriptor.channels, Some(value) if value > 0) {
                problems.push("channels is absent or zero".to_owned());
            }
            if descriptor.block_samples.is_none()
                || descriptor.output_offset_samples.is_none()
                || descriptor.output_samples.is_none()
            {
                problems.push("typed output geometry is incomplete".to_owned());
            }

            match descriptor.codec_metadata.as_deref() {
                None => problems.push("codecMetadata is absent".to_owned()),
                Some(bytes) if bytes.is_empty() => {
                    problems.push("codecMetadata is empty".to_owned())
                }
                Some(bytes) => {
                    if let Err(error) = serde_json::from_slice::<serde_json::Value>(bytes) {
                        problems.push(format!("codecMetadata is not valid JSON: {error}"));
                    }
                }
            }

            checks.push(if problems.is_empty() {
                SpecCheck::pass(
                    format!("Descriptor {index} canonical ECDC shape"),
                    "codec, sample format, output geometry and codecMetadata are present",
                )
            } else {
                SpecCheck::fail(
                    format!("Descriptor {index} canonical ECDC shape"),
                    problems.join("; "),
                )
            });
        }

        if descriptor
            .container
            .eq_ignore_ascii_case(PAYLOAD_CONTAINER_GAP)
        {
            let mut problems = Vec::new();

            if !matches!(
                descriptor.codec.as_deref(),
                Some(codec) if codec.eq_ignore_ascii_case("GAP")
            ) {
                problems.push("codec is absent or not GAP".to_owned());
            }
            if !matches!(descriptor.sample_rate, Some(value) if value > 0) {
                problems.push("sampleRate is absent or zero".to_owned());
            }
            if !matches!(descriptor.channels, Some(value) if value > 0) {
                problems.push("channels is absent or zero".to_owned());
            }
            if descriptor.block_samples.is_some()
                || descriptor.output_offset_samples.is_some()
                || descriptor.output_samples.is_some()
            {
                problems.push("GAP descriptor incorrectly contains output geometry".to_owned());
            }
            if descriptor.codec_metadata.is_some() {
                problems.push("GAP descriptor incorrectly contains codecMetadata".to_owned());
            }

            checks.push(if problems.is_empty() {
                SpecCheck::pass(
                    format!("Descriptor {index} canonical GAP shape"),
                    "container GAP, codec GAP, sample rate/channels present, no geometry",
                )
            } else {
                SpecCheck::fail(
                    format!("Descriptor {index} canonical GAP shape"),
                    problems.join("; "),
                )
            });
        }
    }

    if let Some(entries) = resolved {
        for entry in entries {
            let descriptor = match parsed
                .metadata
                .payload_descriptors
                .get(entry.payload_descriptor_index as usize)
            {
                Some(descriptor) => descriptor,
                None => {
                    checks.push(SpecCheck::fail(
                        format!("Entry {} descriptor reference", entry.index),
                        "descriptor index is out of range",
                    ));
                    continue;
                }
            };

            let end = match entry.byte_offset.checked_add(entry.byte_length) {
                Some(end) => end,
                None => {
                    checks.push(SpecCheck::fail(
                        format!("Entry {} byte range", entry.index),
                        "byte range overflow",
                    ));
                    continue;
                }
            };

            let bytes = match payload.get(entry.byte_offset..end) {
                Some(bytes) => bytes,
                None => {
                    checks.push(SpecCheck::fail(
                        format!("Entry {} byte range", entry.index),
                        "entry exceeds stored payload",
                    ));
                    continue;
                }
            };

            if descriptor
                .container
                .eq_ignore_ascii_case(PAYLOAD_CONTAINER_GAP)
            {
                checks.push(match gap::validate_gap_payload(bytes) {
                    Ok(header) => SpecCheck::pass(
                        format!("Entry {} GAP1 payload", entry.index),
                        format!(
                            "{} samples, {} bytes, seed 0x{:08x}",
                            header.sample_count, header.payload_byte_length, header.seed
                        ),
                    ),
                    Err(error) => SpecCheck::fail(
                        format!("Entry {} GAP1 payload", entry.index),
                        format!("{error:#}"),
                    ),
                });
            }

            if descriptor
                .container
                .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
            {
                checks.push(
                    match record_core::ecdc::headerless_entry_sample_count(bytes, descriptor) {
                        Ok(0) => SpecCheck::fail(
                            format!("Entry {} exact ECDC sample count", entry.index),
                            "musical ECDC entry resolved to zero samples",
                        ),
                        Ok(samples) => SpecCheck::pass(
                            format!("Entry {} exact ECDC sample count", entry.index),
                            format!("{samples} samples"),
                        ),
                        Err(error) => SpecCheck::fail(
                            format!("Entry {} exact ECDC sample count", entry.index),
                            format!("{error:#}"),
                        ),
                    },
                );
            }
        }
    }

    checks.push(
        match record_core::build_programme_map(parsed, Some(record_profile)) {
            Ok(map) => SpecCheck::pass(
                "Pre-decode programme map",
                format!(
                    "{} samples across {} regions",
                    map.total_samples,
                    map.regions.len()
                ),
            ),
            Err(error) => SpecCheck::fail("Pre-decode programme map", format!("{error:#}")),
        },
    );

    checks
}

fn report_spec_consistency(
    out: &mut String,
    parsed: &record_core::RecordStream,
    record_profile: &str,
) -> Result<()> {
    section(out, "CHECK RESULTS");

    let checks = collect_spec_checks(parsed, record_profile);
    let mut omitted_entry_passes = 0usize;

    for check in &checks {
        let repetitive_entry_pass = check.passed
            && check.label.starts_with("Entry ")
            && (check.label.ends_with(" exact ECDC sample count")
                || check.label.ends_with(" GAP1 payload"));

        if repetitive_entry_pass {
            omitted_entry_passes += 1;
            continue;
        }

        writeln!(
            out,
            "  {} {}",
            status_mark(check.passed),
            check.label
        )?;
        writeln!(out, "      {}", check.detail)?;
    }

    if omitted_entry_passes > 0 {
        writeln!(
            out,
            "  {} {omitted_entry_passes} repetitive per-entry checks passed and were omitted",
            green_tick()
        )?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigningCoverage {
    IndirectlySignedUnchecked,
    Unsigned,
}

impl SigningCoverage {
    fn as_str(self) -> &'static str {
        match self {
            Self::IndirectlySignedUnchecked => "signed via release commitment (unchecked)",
            Self::Unsigned => "unsigned",
        }
    }
}

fn report_signing_state(
    out: &mut String,
    descriptor: &RecordDescriptor,
    stream: &[u8],
    manifest: Option<ExternalManifest<'_>>,
) -> Result<()> {
    section(out, "SIGNING STATE");
    let has_signed_reference = descriptor.signed_release_reference.is_some();
    let has_canonical_url = descriptor.canonical_url.is_some();
    let yl_issuance_state = if has_signed_reference && has_canonical_url {
        "final YL issuance markers present"
    } else if has_signed_reference || has_canonical_url {
        "partial YL issuance markers present"
    } else {
        "no YL issuance markers present"
    };

    let signed_reference_state = match descriptor.signed_release_reference.as_ref() {
        None => "absent".to_owned(),
        Some(reference) => match reference.validate() {
            Ok(()) => {
                "present; structurally valid; cryptographic verification not checked".to_owned()
            }
            Err(error) => format!("present; structurally invalid ({error:#})"),
        },
    };

    writeln!(out, "  signed release reference: {signed_reference_state}")?;
    writeln!(out, "  YL issuance state:        {yl_issuance_state}")?;
    writeln!(
        out,
        "  release commitment hash:  {}",
        if has_signed_reference {
            "present in signed reference"
        } else {
            "absent"
        }
    )?;
    writeln!(
        out,
        "  external manifest:        {}",
        if manifest.is_some() {
            "present; displayed for inspection only"
        } else {
            "absent"
        }
    )?;
    writeln!(
        out,
        "  BRS1 stream bytes:        {}",
        if has_signed_reference {
            format!(
                "{}; structural parsing passed ({} bytes)",
                SigningCoverage::IndirectlySignedUnchecked.as_str(),
                stream.len()
            )
        } else {
            format!(
                "{}; structural parsing passed ({} bytes)",
                SigningCoverage::Unsigned.as_str(),
                stream.len()
            )
        }
    )?;

    report_field_signing_state(
        out,
        "record_profile",
        descriptor.record_profile.as_str(),
        if has_signed_reference {
            SigningCoverage::IndirectlySignedUnchecked
        } else {
            SigningCoverage::Unsigned
        },
        if has_signed_reference {
            "valid canonical profile code path; would be covered indirectly by the signed release commitment"
        } else {
            "valid canonical profile code path; not signed in this file"
        },
    )?;
    report_field_signing_state(
        out,
        "payload_encoding",
        descriptor.payload_encoding.as_str(),
        if has_signed_reference {
            SigningCoverage::IndirectlySignedUnchecked
        } else {
            SigningCoverage::Unsigned
        },
        if has_signed_reference {
            "valid canonical payload-encoding code path; would be covered indirectly by the signed release commitment"
        } else {
            "valid canonical payload-encoding code path; not signed in this file"
        },
    )?;
    report_optional_text_field_signing_state(
        out,
        "title",
        descriptor.title.as_deref(),
        SigningCoverage::Unsigned,
        "decoded and UTF-8 validated",
    )?;
    report_optional_text_field_signing_state(
        out,
        "artist",
        descriptor.artist.as_deref(),
        SigningCoverage::Unsigned,
        "decoded and UTF-8 validated",
    )?;
    report_release_id_signing_state(out, descriptor.release_id, has_signed_reference)?;
    report_optional_text_field_signing_state(
        out,
        "catalog_number",
        descriptor.catalog_number.as_deref(),
        SigningCoverage::Unsigned,
        "decoded and UTF-8 validated",
    )?;
    report_optional_text_field_signing_state(
        out,
        "label",
        descriptor.label.as_deref(),
        SigningCoverage::Unsigned,
        "decoded and UTF-8 validated",
    )?;
    report_optional_text_field_signing_state(
        out,
        "artwork_credit",
        descriptor.artwork_credit.as_deref(),
        SigningCoverage::Unsigned,
        "decoded and UTF-8 validated",
    )?;
    report_optional_text_field_signing_state(
        out,
        "canonical_url",
        descriptor.canonical_url.as_deref(),
        SigningCoverage::Unsigned,
        "decoded and UTF-8 validated",
    )?;
    report_optional_number_field_signing_state(
        out,
        "created_at",
        descriptor.created_at,
        SigningCoverage::Unsigned,
        "decoded as descriptor metadata",
    )?;
    report_bytes_field_signing_state(
        out,
        "bsc_pointer",
        descriptor.bsc_pointer.as_deref(),
        SigningCoverage::Unsigned,
        "opaque bytes only; not validated by test-spin",
    )?;

    Ok(())
}

fn report_field_signing_state(
    out: &mut String,
    label: &str,
    value: &str,
    coverage: SigningCoverage,
    validity: &str,
) -> Result<()> {
    writeln!(
        out,
        "  {label}: {} | {} | value={value:?}",
        coverage.as_str(),
        validity
    )?;
    Ok(())
}

fn report_optional_text_field_signing_state(
    out: &mut String,
    label: &str,
    value: Option<&str>,
    coverage: SigningCoverage,
    validity: &str,
) -> Result<()> {
    let presence = match value {
        Some(value) => format!("present | value={value:?}"),
        None => "absent".to_owned(),
    };
    writeln!(
        out,
        "  {label}: {} | {} | {presence}",
        coverage.as_str(),
        validity
    )?;
    Ok(())
}

fn report_optional_number_field_signing_state(
    out: &mut String,
    label: &str,
    value: Option<u64>,
    coverage: SigningCoverage,
    validity: &str,
) -> Result<()> {
    let presence = match value {
        Some(value) => format!("present | value={value}"),
        None => "absent".to_owned(),
    };
    writeln!(
        out,
        "  {label}: {} | {} | {presence}",
        coverage.as_str(),
        validity
    )?;
    Ok(())
}

fn report_bytes_field_signing_state(
    out: &mut String,
    label: &str,
    value: Option<&[u8]>,
    coverage: SigningCoverage,
    validity: &str,
) -> Result<()> {
    let presence = match value {
        Some(value) => format!("present | {} bytes", value.len()),
        None => "absent".to_owned(),
    };
    writeln!(
        out,
        "  {label}: {} | {} | {presence}",
        coverage.as_str(),
        validity
    )?;
    Ok(())
}

fn report_release_id_signing_state(
    out: &mut String,
    value: Option<[u8; record_descriptor::RELEASE_ID_LENGTH]>,
    has_signed_reference: bool,
) -> Result<()> {
    let presence = match value {
        Some(bytes) => format!(
            "present | value={}",
            record_descriptor::release_id_to_text(bytes)
        ),
        None => "absent".to_owned(),
    };
    writeln!(
        out,
        "  release_id: {} | {} | {presence}",
        if has_signed_reference {
            SigningCoverage::IndirectlySignedUnchecked.as_str()
        } else {
            SigningCoverage::Unsigned.as_str()
        },
        if has_signed_reference {
            "canonical raw 16-byte BRD1 field; would be covered indirectly by the signed release commitment"
        } else {
            "canonical raw 16-byte BRD1 field; not signed in this file"
        },
    )?;
    Ok(())
}

fn report_entries(
    out: &mut String,
    parsed: &record_core::RecordStream,
    options: &InspectionOptions<'_>,
) -> Result<()> {
    section(out, "BRS1 PAYLOAD BODIES");

    let payload = record_core::chunk_stream_payload_bytes(parsed);
    let entries = validate_payload_entries_metadata(&parsed.metadata, Some(payload.len()))?;

    writeln!(out, "  reconstructed payload bytes: {}", payload.len())?;
    writeln!(out, "  logical payload entries:     {}", entries.len())?;

    let display_entries = track_boundary_entry_indices(&parsed.metadata);

    let mut previous_printed_index = None;

    for entry_index in display_entries {
        if let Some(previous_index) = previous_printed_index {
            let omitted = entry_index.saturating_sub(previous_index + 1);
            if omitted > 0 {
                writeln!(
                    out,
                    "  ... {omitted} intermediate payload entries omitted; use --verbose to show all"
                )?;
            }
        }

        let entry = &entries[entry_index];
        let bytes = payload_entry_bytes(&payload, entry)?;

        section(
            out,
            &format!("PAYLOAD BODY {} / {}", entry.index + 1, entries.len()),
        );

        writeln!(out, "  entry index:              {}", entry.index)?;
        writeln!(out, "  byte offset:              {}", entry.byte_offset)?;
        writeln!(out, "  byte length:              {}", entry.byte_length)?;
        writeln!(
            out,
            "  descriptor index:         {}",
            entry.payload_descriptor_index
        )?;

        let descriptor = parsed
            .metadata
            .payload_descriptors
            .get(entry.payload_descriptor_index as usize)
            .context("payload descriptor index is out of range")?;

        if descriptor
            .container
            .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
        {
            writeln!(out, "  payload prefix:           headerless codec body")?;
        } else {
            writeln!(out, "  first four bytes:         {:?}", ascii_magic(bytes))?;
        }

        if bytes.get(..4) == Some(gap::GAP_MAGIC.as_slice()) {
            report_gap_payload_body(out, bytes)?;
        } else {
            writeln!(out, "  body prefix:")?;
            writeln!(
                out,
                "{}",
                indent(&hex_prefix(bytes, options.max_hex_bytes.max(384)), 4)
            )?;
        }

        previous_printed_index = Some(entry_index);
    }

    Ok(())
}

fn payload_entry_bytes<'a>(payload: &'a [u8], entry: &ResolvedPayloadEntry) -> Result<&'a [u8]> {
    let end = entry
        .byte_offset
        .checked_add(entry.byte_length)
        .context("payload entry range overflow")?;

    payload
        .get(entry.byte_offset..end)
        .context("payload entry range exceeds reconstructed payload")
}

fn report_gap_payload_body(out: &mut String, entry: &[u8]) -> Result<()> {
    let header = gap::validate_gap_payload(entry).context("invalid GAP payload")?;
    let filler_bytes = entry.len().saturating_sub(gap::GAP_HEADER_LENGTH);

    writeln!(out, "  GAP1 payload:")?;
    writeln!(out, "    encoded bytes:         {}", entry.len())?;
    writeln!(
        out,
        "    declared payload bytes: {}",
        header.payload_byte_length
    )?;
    writeln!(out, "    sample count:          {}", header.sample_count)?;
    writeln!(out, "    filler seed:           0x{:08x}", header.seed)?;
    writeln!(out, "    filler bytes:          {filler_bytes}")?;

    Ok(())
}

pub fn load_bundle_metadata(
    path: impl AsRef<std::path::Path>,
) -> Result<encodec_rs::metadata::OnnxFrameBundleMetadata> {
    let path = path.as_ref();
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read bundle JSON {}", path.display()))?;

    serde_json::from_str(&json).context("failed to deserialize OnnxFrameBundleMetadata")
}

fn report_json_structure(out: &mut String, value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Object(object) => {
            writeln!(out, "  JSON root:               object")?;
            writeln!(out, "  top-level fields:        {}", object.len())?;

            if !object.is_empty() {
                writeln!(out, "  field names:")?;
                for (name, field_value) in object {
                    writeln!(out, "    {name}: {}", json_value_kind(field_value))?;
                }
            }
        }
        serde_json::Value::Array(array) => {
            writeln!(out, "  JSON root:               array")?;
            writeln!(out, "  array items:             {}", array.len())?;

            if let Some(first) = array.first() {
                writeln!(out, "  first item type:         {}", json_value_kind(first))?;
            }
        }
        other => {
            writeln!(out, "  JSON root:               {}", json_value_kind(other))?;
        }
    }

    Ok(())
}

fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn report_final_summary(
    out: &mut String,
    png: &[u8],
    descriptor: &RecordDescriptor,
    stream: &[u8],
    record_profile: &str,
) -> Result<()> {
    let parsed = parse_record_stream(stream)?;
    let checks = collect_spec_checks(&parsed, record_profile);
    let passed = checks.iter().filter(|check| check.passed).count();
    let failed = checks.len().saturating_sub(passed);
    let signed_reference_valid = descriptor
        .signed_release_reference
        .as_ref()
        .map(|reference| reference.validate())
        .transpose()
        .is_ok();
    let final_issuance_markers = descriptor.signed_release_reference.is_some()
        && descriptor.canonical_url.is_some()
        && descriptor.release_id.is_some();
    let overall_ok = failed == 0 && signed_reference_valid;
    let track_count = parsed.metadata.tracks.len();
    let payload_entries = parsed.metadata.payload_entries.len();
    let transport_chunks = parsed.chunks.len();
    let release_id = descriptor
        .release_id
        .map(record_descriptor::release_id_to_text)
        .unwrap_or_else(|| "absent".to_owned());
    let canonical_url = descriptor
        .canonical_url
        .as_deref()
        .unwrap_or("absent");
    let catalogue_code = descriptor
        .canonical_url
        .as_deref()
        .and_then(yl_catalogue_code_from_url)
        .unwrap_or_else(|| "absent".to_owned());
    let signature_key = descriptor
        .signed_release_reference
        .as_ref()
        .and_then(|reference| printable_utf8(&reference.key_id))
        .unwrap_or("absent");

    section(out, "SUMMARY");

    writeln!(
        out,
        "  {} {}",
        if overall_ok { green_tick() } else { red_cross() },
        if overall_ok {
            "VALID BITNEEDLE RECORD"
        } else {
            "RECORD HAS FAILURES"
        }
    )?;
    writeln!(
        out,
        "  {} format checks: {passed}/{} passed",
        if failed == 0 { green_tick() } else { red_cross() },
        checks.len()
    )?;
    writeln!(
        out,
        "  {} YL issuance: {}",
        if final_issuance_markers { green_tick() } else { red_cross() },
        if final_issuance_markers {
            "final markers present"
        } else {
            "incomplete or absent"
        }
    )?;
    writeln!(
        out,
        "  {} signed reference: {}",
        if signed_reference_valid && descriptor.signed_release_reference.is_some() {
            green_tick()
        } else {
            red_cross()
        },
        match descriptor.signed_release_reference.as_ref() {
            Some(_) if signed_reference_valid => "structurally valid; cryptographic verification not performed",
            Some(_) => "structurally invalid",
            None => "absent",
        }
    )?;

    writeln!(out)?;
    writeln!(out, "  RECORD")?;
    writeln!(out, "    title:             {}", descriptor.title.as_deref().unwrap_or("absent"))?;
    writeln!(out, "    artist:            {}", descriptor.artist.as_deref().unwrap_or("absent"))?;
    writeln!(out, "    profile:           {}", descriptor.record_profile)?;
    writeln!(out, "    payload encoding:  {}", descriptor.payload_encoding)?;
    writeln!(out, "    PNG bytes:         {}", png.len())?;
    writeln!(out, "    BRS1 bytes:        {}", stream.len())?;
    writeln!(out, "    tracks:            {track_count}")?;
    writeln!(out, "    payload entries:   {payload_entries}")?;
    writeln!(out, "    transport chunks:  {transport_chunks}")?;

    writeln!(out)?;
    writeln!(out, "  IDENTITY")?;
    writeln!(out, "    release ID:        {release_id}")?;
    writeln!(out, "    YL catalogue code: {catalogue_code}")?;
    writeln!(out, "    canonical URL:     {canonical_url}")?;
    writeln!(
        out,
        "    catalog number:    {}",
        descriptor.catalog_number.as_deref().unwrap_or("absent")
    )?;
    writeln!(
        out,
        "    created at (ms):   {}",
        format_optional_number(descriptor.created_at)
    )?;

    writeln!(out)?;
    writeln!(out, "  SIGNATURE")?;
    writeln!(out, "    key ID:            {signature_key}")?;
    writeln!(
        out,
        "    commitment:        {}",
        descriptor
            .signed_release_reference
            .as_ref()
            .map(|reference| hex::encode(reference.release_commitment_sha256))
            .unwrap_or_else(|| "absent".to_owned())
    )?;
    writeln!(
        out,
        "    signature bytes:   {}",
        descriptor
            .signed_release_reference
            .as_ref()
            .map(|reference| reference.signature.len().to_string())
            .unwrap_or_else(|| "absent".to_owned())
    )?;
    writeln!(
        out,
        "    BSC pointer:       {}",
        descriptor
            .bsc_pointer
            .as_ref()
            .map(|pointer| format!("present ({} bytes)", pointer.len()))
            .unwrap_or_else(|| "absent".to_owned())
    )?;

    if failed > 0 {
        writeln!(out)?;
        writeln!(out, "  {} {failed} check(s) failed; see CHECK RESULTS above", red_cross())?;
    }

    Ok(())
}

fn format_optional_text(value: Option<&str>) -> String {
    value
        .map(|text| format!("{text:?}"))
        .unwrap_or_else(|| "absent".to_owned())
}

fn format_optional_number<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "absent".to_owned())
}

fn yl_catalogue_code_from_url(url: &str) -> Option<String> {
    let slug = url
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .trim();

    if slug.is_empty() {
        return None;
    }

    let compact = slug
        .chars()
        .filter(|character| *character != '-')
        .collect::<String>()
        .to_ascii_uppercase();

    if compact.len() != 11 || !compact.chars().all(|character| character.is_ascii_alphanumeric()) {
        return None;
    }

    Some(format!("yl_{compact}"))
}

fn green_tick() -> &'static str {
    "\x1b[1;32m✓\x1b[0m"
}

fn red_cross() -> &'static str {
    "\x1b[1;31m✗\x1b[0m"
}

fn status_mark(passed: bool) -> &'static str {
    if passed {
        green_tick()
    } else {
        red_cross()
    }
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

    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
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
    if png.len() < 33 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }

    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);

    Some((width, height, png[24], png[25]))
}

#[allow(dead_code)]
fn container_code_name(code: u8) -> &'static str {
    match code {
        CONTAINER_ECDC => "ECDC",
        CONTAINER_MOSS_NANO => "MOSSNANO",
        CONTAINER_EXTENSION => "EXTENSION",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod programme_summary_tests {
    use super::*;
    use record_core::{
        build_programme_map, Chunk, PayloadEntryDescriptor, RecordStream, TrackDescriptor,
        TrackGapDescriptor,
    };

    fn ecdc_descriptor() -> record_core::PayloadDescriptor {
        record_core::ecdc::ecdc_payload_descriptor(
            48_000,
            2,
            &record_core::ecdc::EcdcCodecMetadata {
                model: "encodec_48khz".to_owned(),
                num_codebooks: 8,
                lm: true,
                fp_scale: 8192,
                min_range: 2,
                bitstream_version: 2,
                lm_frame_length: 203,
            },
        )
        .unwrap()
    }

    /// Three musical tracks (60 + 60 + 61 = 181 entries) separated by two
    /// explicit two-revolution track gaps (4 entries): 185 entries total, no
    /// GAP container or GAP1 payload anywhere — every entry is ordinary ECDC.
    fn three_tracks_two_gaps_record_stream() -> RecordStream {
        let entry = vec![0xABu8; 16];
        let entry_count = 60 + 2 + 60 + 2 + 61;
        let mut payload = Vec::with_capacity(entry_count * entry.len());
        let mut payload_entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            payload.extend_from_slice(&entry);
            payload_entries.push(PayloadEntryDescriptor {
                byte_length: entry.len(),
                payload_descriptor_index: 0,
            });
        }

        let metadata = RecordStreamMetadata {
            version: record_core::RECORD_STREAM_METADATA_VERSION,
            encrypted: false,
            payload_descriptors: vec![ecdc_descriptor()],
            payload_entries,
            tracks: vec![
                TrackDescriptor {
                    title: "Track A".to_owned(),
                    first_revolution_index: 0,
                    revolution_count: 60,
                },
                TrackDescriptor {
                    title: "Track B".to_owned(),
                    first_revolution_index: 62,
                    revolution_count: 60,
                },
                TrackDescriptor {
                    title: "Track C".to_owned(),
                    first_revolution_index: 124,
                    revolution_count: 61,
                },
            ],
            track_gaps: vec![
                TrackGapDescriptor {
                    first_revolution_index: 60,
                    revolution_count: 2,
                    after_track_index: 0,
                },
                TrackGapDescriptor {
                    first_revolution_index: 122,
                    revolution_count: 2,
                    after_track_index: 1,
                },
            ],
        };

        RecordStream {
            metadata,
            metadata_bytes: Vec::new(),
            chunks: vec![Chunk {
                payload,
                crc32: 0,
                nonce: None,
            }],
        }
    }

    #[test]
    fn summary_reports_musical_and_track_gap_entries_separately() {
        let stream = three_tracks_two_gaps_record_stream();
        let mut out = String::new();
        report_stream_summary(&mut out, &stream).unwrap();

        assert!(out.contains("tracks:                    3"), "{out}");
        assert!(out.contains("TrackGap ranges:           2"), "{out}");
        assert!(out.contains("musical timeline entries:  181"), "{out}");
        assert!(out.contains("TrackGap timeline entries: 4"), "{out}");
        assert!(out.contains("total timeline entries:    185"), "{out}");
    }

    #[test]
    fn record_passes_pre_decode_programme_map_validation() {
        let stream = three_tracks_two_gaps_record_stream();
        validate_track_listing_metadata(&stream.metadata).unwrap();
        let map = build_programme_map(&stream, Some("single45")).unwrap();
        // 3 track regions + 4 gap regions (gap regions are not merged across
        // consecutive entries, unlike same-track regions): 7 total.
        assert_eq!(map.regions.len(), 7);
        assert_eq!(
            map.total_samples,
            185 * u64::from(record_core::ecdc::ECDC_OUTPUT_SAMPLES)
        );
    }
}
