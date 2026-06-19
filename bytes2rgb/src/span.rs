//! Byte-offset tone spans used by the decoder.

use crate::{TonedConfig, TonedPalette};
use anyhow::{bail, Context, Result};

/// The resolved settings for one encoded byte range. Persist these (e.g. as
/// track-level metadata) — they are everything a decoder needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToneSpan {
    pub byte_offset: usize,
    pub byte_length: usize,
    pub pixel_offset: usize,
    pub pixel_count: usize,
    pub config: TonedConfig,
}

/// Validates that `spans` form a single contiguous, self-consistent cover from
/// byte/pixel offset 0, with each span's pixel count matching its byte length
/// at its `bits_per_pixel`. Returns `(total_byte_length, total_pixel_count)`.
pub fn validate_tone_spans(spans: &[ToneSpan]) -> Result<(usize, usize)> {
    if spans.is_empty() {
        bail!("tone spans must not be empty");
    }

    let mut byte_cursor = 0usize;
    let mut pixel_cursor = 0usize;
    for (index, span) in spans.iter().enumerate() {
        if span.byte_offset != byte_cursor {
            bail!(
                "tone span {index} byte offset {} is not contiguous; expected {byte_cursor}",
                span.byte_offset
            );
        }
        if span.pixel_offset != pixel_cursor {
            bail!(
                "tone span {index} pixel offset {} is not contiguous; expected {pixel_cursor}",
                span.pixel_offset
            );
        }
        if span.byte_length == 0 {
            bail!("tone span {index} has zero byte length");
        }
        if !(1..=24).contains(&span.config.bits_per_pixel) {
            bail!(
                "tone span {index} bits_per_pixel {} must be between 1 and 24",
                span.config.bits_per_pixel
            );
        }

        let bit_length = span
            .byte_length
            .checked_mul(8)
            .context("tone span bit length overflow")?;
        let expected_pixels = bit_length.div_ceil(span.config.bits_per_pixel as usize);
        if span.pixel_count != expected_pixels {
            bail!(
                "tone span {index} pixel count {} does not match {} bytes at {} bits/pixel \
                 (expected {expected_pixels})",
                span.pixel_count,
                span.byte_length,
                span.config.bits_per_pixel,
            );
        }

        byte_cursor = byte_cursor
            .checked_add(span.byte_length)
            .context("total tone span byte length overflow")?;
        pixel_cursor = pixel_cursor
            .checked_add(span.pixel_count)
            .context("total tone span pixel count overflow")?;
    }

    Ok((byte_cursor, pixel_cursor))
}

/// Decodes toned pixels using the persisted span settings. Fully transparent
/// pixels (e.g. trailing square-PNG padding) are skipped before spans are
/// applied. Spans are validated for contiguity and exact pixel/byte coverage
/// before any palette decoding, and the recovered length is checked afterward.
pub fn decode_toned_spans(rgba: &[u8], spans: &[ToneSpan]) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let (expected_byte_length, _expected_pixel_count) = validate_tone_spans(spans)?;

    let opaque: Vec<u8> = rgba
        .chunks_exact(4)
        .filter(|chunk| chunk[3] != 0)
        .flatten()
        .copied()
        .collect();

    let mut bytes = Vec::with_capacity(expected_byte_length);

    for (span_index, span) in spans.iter().enumerate() {
        let start = span
            .pixel_offset
            .checked_mul(4)
            .context("tone span pixel start overflow")?;
        let pixel_end = span
            .pixel_offset
            .checked_add(span.pixel_count)
            .context("tone span pixel range overflow")?;
        let end = pixel_end
            .checked_mul(4)
            .context("tone span RGBA end overflow")?;

        if end > opaque.len() {
            bail!(
                "tone span {span_index} pixel range {}..{} exceeds extracted payload \
                 pixel count {}; byte range {}..{}; bits_per_pixel={}; base=#{:02X}{:02X}{:02X}; \
                 luma_tolerance={}; ordering={:?}",
                span.pixel_offset,
                pixel_end,
                opaque.len() / 4,
                span.byte_offset,
                span.byte_offset + span.byte_length,
                span.config.bits_per_pixel,
                span.config.base[0],
                span.config.base[1],
                span.config.base[2],
                span.config.luma_tolerance,
                span.config.ordering,
            );
        }

        let palette = TonedPalette::shared(span.config)?;
        bytes.extend(palette.rgba_to_bytes(&opaque[start..end], Some(span.byte_length))?);
    }

    if bytes.len() != expected_byte_length {
        bail!(
            "toned decode produced {} bytes, expected {expected_byte_length}",
            bytes.len(),
        );
    }

    Ok(bytes)
}
