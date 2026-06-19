use anyhow::{bail, Context, Result};
use bytes2rgb::rgba_to_bytes as track_rgba_to_bytes;
use record_core::{
    build_header_spiral_indices, build_spiral_mask, build_trailer_spiral_indices,
    known_record_profile_names, normalize_record_profile_name, RECORD_STREAM_MAGIC,
};
use record_descriptor::RecordDescriptor;

pub const PAYLOAD_ENCODING_TONED_V1: &str = "toned-v1";

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

    let payload_len =
        u16::from_be_bytes(prefix[5..7].try_into().expect("slice length")) as usize;

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

    let descriptor_bytes = record_descriptor::metadata_bytes_from_grayscale_rgba(
        rgba,
        &descriptor_indices,
        payload_len,
        "record descriptor",
    )?;

    record_descriptor::decode_record_descriptor_bytes(&descriptor_bytes)
}

fn decode_record_groove_to_track_data(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    b_value: f64,
) -> Result<(Vec<u8>, usize)> {
    let expected_rgba_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("record dimensions overflow")?;

    if rgba.len() != expected_rgba_len {
        bail!("record RGBA length does not match width * height * 4");
    }

    let mask =
        build_spiral_mask(width, height, b_value, record_profile, None, None, None)?;
    let mut track_data =
        Vec::with_capacity(mask.ordered_pixel_indices.len().saturating_mul(4));

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
    }

    if track_data.is_empty() {
        bail!("groove-ordered decode found no written groove pixels");
    }

    let pixel_count = track_data.len() / 4;

    Ok((track_data, pixel_count))
}

pub fn infer_record_profile_from_png(png_bytes: &[u8]) -> Result<String> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;

    for &candidate in known_record_profile_names() {
        if let Ok(descriptor) =
            decode_record_descriptor_from_rgba(&rgba, width, height, candidate)
        {
            let normalized =
                normalize_record_profile_name(&descriptor.record_profile)?;

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

    let descriptor =
        decode_record_descriptor_from_rgba(&rgba, width, height, &normalized_profile)?;

    let normalized_descriptor_profile =
        normalize_record_profile_name(&descriptor.record_profile)?;

    if normalized_descriptor_profile != normalized_profile {
        bail!(
            "record descriptor profile {} does not match inferred profile {}",
            normalized_descriptor_profile,
            normalized_profile
        );
    }

    Ok((normalized_profile, descriptor))
}

pub fn decode_record_png_to_chunk_stream_for_profile_with_length(
    png_bytes: &[u8],
    record_profile: &str,
    byte_length: Option<usize>,
) -> Result<DecodedChunkStream> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;
    let normalized_profile = normalize_record_profile_name(record_profile)?;
    let descriptor =
        decode_record_descriptor_from_rgba(&rgba, width, height, &normalized_profile)?;
    let resolved_byte_length = byte_length.or(descriptor.stream_byte_length);

    let (track_data, pixel_count) = decode_record_groove_to_track_data(
        &rgba,
        width,
        height,
        &normalized_profile,
        descriptor.b_value(),
    )?;

    let bytes = match descriptor.payload_encoding.as_str() {
        record_core::PAYLOAD_ENCODING_RGB => {
            track_rgba_to_bytes(&track_data, resolved_byte_length)?
        }
        PAYLOAD_ENCODING_TONED_V1 => {
            bail!(
                "toned-v1 records require the removed JSON rgbTone map; \
                 define a binary BRD1/BRS1 tone-map structure before decoding them"
            )
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
    decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        record_profile,
        None,
    )
}

pub fn decode_record_png_to_chunk_stream_with_length(
    png_bytes: &[u8],
    byte_length: Option<usize>,
) -> Result<(String, DecodedChunkStream)> {
    let (profile, descriptor) =
        decode_record_descriptor_from_png(png_bytes, None)?;
    let resolved_byte_length = byte_length.or(descriptor.stream_byte_length);
    let decoded = decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        &profile,
        resolved_byte_length,
    )?;

    Ok((profile, decoded))
}

pub fn decode_record_png_to_chunk_stream(
    png_bytes: &[u8],
) -> Result<(String, DecodedChunkStream)> {
    decode_record_png_to_chunk_stream_with_length(png_bytes, None)
}

pub fn decode_record_png(png_bytes: &[u8]) -> Result<DecodedRecord> {
    let (record_profile, descriptor) =
        decode_record_descriptor_from_png(png_bytes, None)?;
    let chunk_stream =
        decode_record_png_to_chunk_stream_for_profile_with_length(
            png_bytes,
            &record_profile,
            descriptor.stream_byte_length,
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
