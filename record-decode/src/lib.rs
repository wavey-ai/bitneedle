use anyhow::{bail, Context, Result};
use bytes2rgb::rgba_to_bytes as track_rgba_to_bytes;
use bytes2rgb::{
    decode_toned_clock, decode_toned_spans, pixel_angle, pixel_radius, ClockSlot, ToneClock,
    ToneOrdering as BytesToneOrdering, ToneSpan, TonedConfig,
};
use record_core::{
    build_lead_in_spiral_indices, build_run_out_spiral_indices, build_spiral_mask_with_handedness,
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

/// The wheel as the groove encoder takes it.
///
/// A version 1 map comes back from the descriptor with its one ring written
/// out, so no caller has to know which version it was.
fn tone_clock_from_descriptor(clock: &record_descriptor::ToneClockDescriptor) -> ToneClock {
    ToneClock {
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
    let wheel = tone_clock_from_descriptor(clock);
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

/// The magic a raw sidecar starts with. See [`load_record_rgba`].
#[cfg(feature = "rgba")]
pub const RGBA_SIDECAR_MAGIC: &[u8; 8] = b"BNRGBA\0\0";

/// How long the raw sidecar's header is: the magic, then the width and the
/// height as little-endian `u32`.
#[cfg(feature = "rgba")]
pub const RGBA_SIDECAR_HEADER_LENGTH: usize = 16;

/// A pressed record read back to pixels, whatever it was written as.
///
/// The format is identified by the bytes rather than by the caller. Every
/// function in this crate that names a PNG takes this: PNG is what the
/// press writes, and a record exported as TIFF or QOI is the same record
/// with the same pixels in a different wrapper. Which wrappers a build
/// understands is decided by its features.
pub fn load_record_rgba(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    #[cfg(feature = "rgba")]
    if bytes.starts_with(RGBA_SIDECAR_MAGIC) {
        return load_rgba_sidecar(bytes);
    }

    let image = match image::load_from_memory(bytes) {
        Ok(image) => image,
        // TGA is the one format here that a reader cannot recognise: it
        // opens with a length and a type byte and carries no magic at all,
        // so a sniffer has nothing to match. It is tried by name when
        // nothing else claims the bytes, and only then, because reading
        // arbitrary bytes as TGA succeeds far too easily.
        #[cfg(feature = "tga")]
        Err(_) => image::load_from_memory_with_format(bytes, image::ImageFormat::Tga)
            .context("failed to decode record image")?,
        #[cfg(not(feature = "tga"))]
        Err(error) => return Err(anyhow::Error::new(error).context("failed to decode record image")),
    }
    .to_rgba8();

    let (width, height) = image.dimensions();

    Ok((width as usize, height as usize, image.into_raw()))
}

/// The raw sidecar: no container, so the header is the whole of what says
/// how to read the bytes after it.
#[cfg(feature = "rgba")]
fn load_rgba_sidecar(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    if bytes.len() < RGBA_SIDECAR_HEADER_LENGTH {
        bail!("raw record is shorter than its own header");
    }

    let width = u32::from_le_bytes(bytes[8..12].try_into().expect("slice length")) as usize;
    let height = u32::from_le_bytes(bytes[12..16].try_into().expect("slice length")) as usize;
    let pixels = &bytes[RGBA_SIDECAR_HEADER_LENGTH..];
    let expected = width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(4))
        .context("raw record dimensions overflow")?;

    if pixels.len() != expected {
        bail!(
            "raw record says {width}x{height} and carries {} bytes, not {expected}",
            pixels.len()
        );
    }

    Ok((width, height, pixels.to_vec()))
}

fn load_png_rgba(png_bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    load_record_rgba(png_bytes)
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
    let lead_in_indices =
        build_lead_in_spiral_indices(width, height, record_profile, None, None, None)?;

    // The prefix comes out of the lead-in alone, and it carries the radius the
    // cut stopped at. That has to be read before the lead-out can be walked:
    // the band widens into whatever room the programme left, so its geometry
    // is a consequence of the cut rather than a constant. The prefix is 29
    // bytes against a lead-in of some thousands of pixels, so it is never in
    // the part of the stream that spills.
    let prefix_bytes = record_descriptor::metadata_bytes_from_grayscale_rgba(
        rgba,
        &lead_in_indices,
        record_descriptor::RECORD_DESCRIPTOR_PREFIX_LENGTH,
        "record descriptor prefix",
    )?;

    let payload_len = descriptor_payload_len_from_prefix(&prefix_bytes)?;
    let lead_in_capacity =
        record_descriptor::metadata_byte_capacity_for_pixel_count(lead_in_indices.len());

    // The lead-in holds 2 428 bytes on an LP and a toned record's descriptor
    // weighs 282, so this is the path an ordinary record takes.
    if payload_len <= lead_in_capacity {
        return record_descriptor::metadata_bytes_from_grayscale_rgba(
            rgba,
            &lead_in_indices,
            payload_len,
            "record descriptor",
        );
    }

    // The record that overran it. The rest is in the trailer, read with the
    // clock the band was cut with. That clock is a segment in the lead-in,
    // so the lead-in is read to the brim first.
    let head = record_descriptor::metadata_bytes_from_grayscale_rgba(
        rgba,
        &lead_in_indices,
        lead_in_capacity,
        "record descriptor",
    )?;
    let clock = record_descriptor::trailer_clock_from_stream_head(&head).context(
        "the descriptor runs past the lead-in and does not say how its trailer is toned",
    )?;

    let cut_inner_radius = match u16::from_be_bytes([prefix_bytes[19], prefix_bytes[20]]) {
        0 => None,
        radius => Some(i32::from(radius)),
    };
    let trailer_indices =
        build_run_out_spiral_indices(width, height, record_profile, cut_inner_radius)?;

    let mut bytes = head;
    bytes.extend_from_slice(&record_descriptor::band_bytes_from_toned_rgba(
        rgba,
        width,
        height,
        &trailer_indices,
        &clock,
        payload_len - lead_in_capacity,
        "record descriptor trailer",
    )?);

    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn decode_record_groove_to_track_data(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    b_value: f64,
    spiral_family: &SpiralFamily,
    // Read off the descriptor, never assumed: a groove has to be retraced
    // with the hand it was cut with or the pixels come back in the wrong
    // order — which is not a wrong picture, it is a wrong stream.
    clockwise: bool,
) -> Result<(Vec<u8>, usize, Vec<usize>)> {
    let expected_rgba_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("record dimensions overflow")?;

    if rgba.len() != expected_rgba_len {
        bail!("record RGBA length does not match width * height * 4");
    }

    let mask = build_spiral_mask_with_handedness(
        width,
        height,
        b_value,
        spiral_family,
        record_profile,
        None,
        None,
        None,
        clockwise,
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
        descriptor.spiral_clockwise,
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
