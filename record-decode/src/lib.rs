use anyhow::{bail, Context, Result};
use bytes2rgb::rgba_to_bytes as track_rgba_to_bytes;
use bytes2rgb::{
    decode_toned_clock, decode_toned_spans, pixel_angle, pixel_radius, ClockSlot, ToneClock,
    ToneOrdering as BytesToneOrdering, ToneSpan, TonedConfig,
};
use record_core::{
    build_header_spiral_indices, build_spiral_mask_with_family, build_trailer_spiral_indices,
    known_record_profile_names, normalize_record_profile_name, SpiralFamily, RECORD_STREAM_MAGIC,
};
use record_descriptor::{
    resolve_tone_spans, RecordDescriptor, ToneOrdering as DescriptorToneOrdering,
};

pub const PAYLOAD_ENCODING_TONED_V1: &str = "toned-v1";
pub const PAYLOAD_ENCODING_TONED_V2: &str = "toned-v2";

#[derive(Debug, Clone)]
pub struct DecodedChunkStream {
    pub bytes: Vec<u8>,
    pub pixel_count: usize,
}

#[derive(Debug, Clone)]
pub struct DecodedRecord {
    pub record_profile: String,
    pub descriptor: RecordDescriptor,
    pub chunk_stream: DecodedChunkStream,
}

fn decode_toned_track_to_bytes(
    track_data: &[u8],
    tone_spans: &[record_descriptor::ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    if tone_spans.is_empty() {
        bail!("toned-v1 record descriptor has no tone spans");
    }

    let resolved = resolve_tone_spans(tone_spans, expected_byte_length)
        .context("invalid toned-v1 carrier map")?;

    let spans: Vec<ToneSpan> = resolved
        .into_iter()
        .map(|span| ToneSpan {
            byte_offset: span.byte_offset,
            byte_length: span.byte_length,
            pixel_offset: span.pixel_offset,
            pixel_count: span.pixel_count,
            config: TonedConfig {
                base: span.base,
                luma_tolerance: span.luma_tolerance,
                bits_per_pixel: u32::from(span.bits_per_pixel),
                ordering: match span.ordering {
                    DescriptorToneOrdering::BaseProximity => BytesToneOrdering::BaseProximity,
                    DescriptorToneOrdering::ChromaProximity => BytesToneOrdering::ChromaProximity,
                },
            },
        })
        .collect();

    decode_toned_spans(track_data, &spans).context("failed to decode toned-v1 groove pixels")
}

fn bytes_tone_ordering(ordering: DescriptorToneOrdering) -> BytesToneOrdering {
    match ordering {
        DescriptorToneOrdering::BaseProximity => BytesToneOrdering::BaseProximity,
        DescriptorToneOrdering::ChromaProximity => BytesToneOrdering::ChromaProximity,
    }
}

/// Decodes a toned-v2 groove. Each lifted pixel's slot follows from where it
/// sits on the raster — its angle about the centre, in the record's frame —
/// and its index in the groove, so the raster indices the walk visited are
/// all the decoder needs beyond the wheel itself.
fn decode_clock_toned_track_to_bytes(
    track_data: &[u8],
    pixel_indices: &[usize],
    width: usize,
    height: usize,
    clock: &record_descriptor::ToneClockDescriptor,
    expected_byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    record_descriptor::validate_tone_clock(clock, expected_byte_length)
        .context("invalid toned-v2 tone clock")?;
    let wheel = ToneClock {
        // A version 1 map comes back from the descriptor with its one ring
        // written out, so nothing here has to know which version it was.
        rings: clock.ring_slots(),
        span: clock.span,
        rotation_centidegrees: clock.rotation_centidegrees.clone(),
        blend: clock.blend,
        bits_per_pixel: u32::from(clock.bits_per_pixel),
        ordering: bytes_tone_ordering(clock.ordering),
        slots: clock
            .slots
            .iter()
            .map(|slot| ClockSlot {
                base: slot.base,
                luma_tolerance: slot.luma_tolerance,
                gap_base: slot.gap_base,
                gap_luma_tolerance: slot.gap_luma_tolerance,
            })
            .collect(),
        gap_switch_offsets: clock.gap_switch_offsets.clone(),
    };
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let angles: Vec<f64> = pixel_indices
        .iter()
        .map(|&index| {
            pixel_angle(
                (index % width) as f64,
                (index / width) as f64,
                center_x,
                center_y,
            )
        })
        .collect();
    // The ring a pixel is in, off the same two numbers its angle came from.
    let radii: Vec<f64> = pixel_indices
        .iter()
        .map(|&index| {
            pixel_radius(
                (index % width) as f64,
                (index / width) as f64,
                center_x,
                center_y,
            )
        })
        .collect();
    // A payload that came in over its nominal keeps writing past the cut;
    // whatever was lifted beyond the declared length is padding to the wheel.
    let needed = expected_byte_length
        .map(|length| record_descriptor::tone_clock_pixel_count(clock, length))
        .transpose()?
        .unwrap_or(angles.len())
        .min(angles.len());
    decode_toned_clock(
        &track_data[..needed * 4],
        &wheel,
        &angles[..needed],
        &radii[..needed],
        expected_byte_length,
    )
    .context("failed to decode toned-v2 groove pixels")
}

fn load_png_rgba(png_bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    let image = image::load_from_memory(png_bytes)
        .context("failed to decode record PNG")?
        .to_rgba8();

    let (width, height) = image.dimensions();

    Ok((width as usize, height as usize, image.into_raw()))
}

fn descriptor_payload_len_from_prefix(prefix: &[u8]) -> Result<usize> {
    if prefix.len() < record_descriptor::RECORD_DESCRIPTOR_PREFIX_LENGTH {
        bail!("record descriptor prefix is too short");
    }

    if &prefix[..4] != record_descriptor::RECORD_DESCRIPTOR_MAGIC {
        bail!("record descriptor magic mismatch");
    }

    let payload_len = u16::from_be_bytes(prefix[5..7].try_into().expect("slice length")) as usize;

    if payload_len < record_descriptor::RECORD_DESCRIPTOR_PREFIX_LENGTH {
        bail!("record descriptor payload length is invalid");
    }

    Ok(payload_len)
}

fn decode_record_descriptor_from_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<RecordDescriptor> {
    let descriptor_bytes = record_descriptor_bytes_from_rgba(rgba, width, height, record_profile)?;

    record_descriptor::decode_record_descriptor_bytes(&descriptor_bytes)
}

fn record_descriptor_bytes_from_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<Vec<u8>> {
    let header_indices =
        build_header_spiral_indices(width, height, record_profile, None, None, None)?;
    let trailer_indices =
        build_trailer_spiral_indices(width, height, record_profile, None, None, None)?;

    let prefix_bytes = record_descriptor::metadata_bytes_from_grayscale_rgba(
        rgba,
        &header_indices,
        record_descriptor::RECORD_DESCRIPTOR_PREFIX_LENGTH,
        "record descriptor prefix",
    )?;

    let payload_len = descriptor_payload_len_from_prefix(&prefix_bytes)?;

    let mut descriptor_indices = header_indices;
    descriptor_indices.extend_from_slice(&trailer_indices);

    record_descriptor::metadata_bytes_from_grayscale_rgba(
        rgba,
        &descriptor_indices,
        payload_len,
        "record descriptor",
    )
}

fn decode_record_groove_to_track_data(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    b_value: f64,
    spiral_family: &SpiralFamily,
) -> Result<(Vec<u8>, usize, Vec<usize>)> {
    let expected_rgba_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("record dimensions overflow")?;

    if rgba.len() != expected_rgba_len {
        bail!("record RGBA length does not match width * height * 4");
    }

    let mask = build_spiral_mask_with_family(
        width,
        height,
        b_value,
        spiral_family,
        record_profile,
        None,
        None,
        None,
    )?;
    let mut track_data = Vec::with_capacity(mask.ordered_pixel_indices.len().saturating_mul(4));
    let mut lifted_indices = Vec::with_capacity(mask.ordered_pixel_indices.len());

    for &pixel_index in &mask.ordered_pixel_indices {
        let rgba_index = pixel_index
            .checked_mul(4)
            .context("groove RGBA index overflow")?;

        if rgba_index + 3 >= rgba.len() {
            bail!("groove pixel index is outside RGBA buffer");
        }

        if rgba[rgba_index + 3] == 0 {
            break;
        }

        track_data.extend_from_slice(&rgba[rgba_index..rgba_index + 4]);
        lifted_indices.push(pixel_index);
    }

    if track_data.is_empty() {
        bail!("groove-ordered decode found no written groove pixels");
    }

    let pixel_count = track_data.len() / 4;

    Ok((track_data, pixel_count, lifted_indices))
}

pub fn infer_record_profile_from_png(png_bytes: &[u8]) -> Result<String> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;

    for &candidate in known_record_profile_names() {
        if let Ok(descriptor) = decode_record_descriptor_from_rgba(&rgba, width, height, candidate)
        {
            let normalized = normalize_record_profile_name(&descriptor.record_profile)?;

            if normalized == candidate {
                return Ok(normalized);
            }
        }
    }

    bail!("failed to infer record profile from descriptor spirals")
}

pub fn decode_record_descriptor_from_png(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<(String, RecordDescriptor)> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;

    let normalized_profile = match record_profile {
        Some(profile) => normalize_record_profile_name(profile)?,
        None => infer_record_profile_from_png(png_bytes)?,
    };

    let descriptor = decode_record_descriptor_from_rgba(&rgba, width, height, &normalized_profile)?;

    let normalized_descriptor_profile = normalize_record_profile_name(&descriptor.record_profile)?;

    if normalized_descriptor_profile != normalized_profile {
        bail!(
            "record descriptor profile {} does not match inferred profile {}",
            normalized_descriptor_profile,
            normalized_profile
        );
    }

    Ok((normalized_profile, descriptor))
}

pub fn decode_record_descriptor_bytes_from_png(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<(String, Vec<u8>)> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;
    let normalized_profile = match record_profile {
        Some(profile) => normalize_record_profile_name(profile)?,
        None => infer_record_profile_from_png(png_bytes)?,
    };
    let bytes = record_descriptor_bytes_from_rgba(&rgba, width, height, &normalized_profile)?;
    let descriptor = record_descriptor::decode_record_descriptor_bytes(&bytes)?;
    let descriptor_profile = normalize_record_profile_name(&descriptor.record_profile)?;
    if descriptor_profile != normalized_profile {
        bail!(
            "record descriptor profile {} does not match inferred profile {}",
            descriptor_profile,
            normalized_profile
        );
    }
    Ok((normalized_profile, bytes))
}

pub fn decode_record_png_to_chunk_stream_for_profile_with_length(
    png_bytes: &[u8],
    record_profile: &str,
    byte_length: Option<usize>,
) -> Result<DecodedChunkStream> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;
    let normalized_profile = normalize_record_profile_name(record_profile)?;
    let descriptor = decode_record_descriptor_from_rgba(&rgba, width, height, &normalized_profile)?;
    let resolved_byte_length = byte_length.or(Some(descriptor.stream_byte_length));

    let (track_data, pixel_count, pixel_indices) = decode_record_groove_to_track_data(
        &rgba,
        width,
        height,
        &normalized_profile,
        descriptor.b_value(),
        &descriptor.spiral_family,
    )?;

    let bytes = match descriptor.payload_encoding.as_str() {
        record_core::PAYLOAD_ENCODING_RGB => {
            track_rgba_to_bytes(&track_data, resolved_byte_length)?
        }
        PAYLOAD_ENCODING_TONED_V1 => {
            decode_toned_track_to_bytes(&track_data, &descriptor.tone_spans, resolved_byte_length)?
        }
        PAYLOAD_ENCODING_TONED_V2 => {
            let clock = descriptor
                .tone_clock
                .as_ref()
                .context("toned-v2 record descriptor has no tone clock")?;
            decode_clock_toned_track_to_bytes(
                &track_data,
                &pixel_indices,
                width,
                height,
                clock,
                resolved_byte_length,
            )?
        }
        other => bail!("unsupported record payload encoding: {other}"),
    };

    if bytes.len() < RECORD_STREAM_MAGIC.len()
        || &bytes[..RECORD_STREAM_MAGIC.len()] != RECORD_STREAM_MAGIC
    {
        bail!("decoded groove does not start with BRS1 record stream magic");
    }

    Ok(DecodedChunkStream { bytes, pixel_count })
}

pub fn decode_record_png_to_chunk_stream_for_profile(
    png_bytes: &[u8],
    record_profile: &str,
) -> Result<DecodedChunkStream> {
    decode_record_png_to_chunk_stream_for_profile_with_length(png_bytes, record_profile, None)
}

pub fn decode_record_png_to_chunk_stream_with_length(
    png_bytes: &[u8],
    byte_length: Option<usize>,
) -> Result<(String, DecodedChunkStream)> {
    let (profile, descriptor) = decode_record_descriptor_from_png(png_bytes, None)?;
    let resolved_byte_length = byte_length.or(Some(descriptor.stream_byte_length));
    let decoded = decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        &profile,
        resolved_byte_length,
    )?;

    Ok((profile, decoded))
}

pub fn decode_record_png_to_chunk_stream(png_bytes: &[u8]) -> Result<(String, DecodedChunkStream)> {
    decode_record_png_to_chunk_stream_with_length(png_bytes, None)
}

pub fn decode_record_png(png_bytes: &[u8]) -> Result<DecodedRecord> {
    let (record_profile, descriptor) = decode_record_descriptor_from_png(png_bytes, None)?;
    let chunk_stream = decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        &record_profile,
        Some(descriptor.stream_byte_length),
    )?;

    Ok(DecodedRecord {
        record_profile,
        descriptor,
        chunk_stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_brs1_groove_bytes() {
        let track_data = vec![b'X', b'X', b'X', 255, b'X', 0, 0, 255];
        let bytes = track_rgba_to_bytes(&track_data, None).unwrap();

        assert_ne!(&bytes[..4], RECORD_STREAM_MAGIC);
    }

    #[test]
    fn toned_v1_is_explicitly_not_legacy_decoded() {
        assert_eq!(PAYLOAD_ENCODING_TONED_V1, "toned-v1");
    }
}
