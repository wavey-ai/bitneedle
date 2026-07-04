// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Byte-offset tone spans.
//!
//! A **span** is a contiguous byte range of the payload rendered with one
//! [`TonedConfig`]. The caller asks for colour changes at byte offsets;
//! encoding resolves each colour (via [`TonedConfig::balanced`]) and returns
//! the pixels together with the resolved settings per span, which the caller
//! persists as track-level metadata (e.g. in the chunk stream). Decoding
//! takes those persisted spans back — nothing is hidden in the pixels, so
//! the data survives any pixel-exact capture of the visual spectrum.

use crate::{TonedConfig, TonedPalette};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;

/// A colour change request: from `byte_offset` (until the next request, or
/// the end of the stream) render with `base`. The first request must start
/// at offset 0.
#[derive(Debug, Clone, Copy)]
pub struct ToneRequest {
    pub byte_offset: usize,
    pub base: [u8; 3],
}

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

/// Result of [`encode_toned_spans`]: the pixels plus the settings to persist.
#[derive(Debug, Clone)]
pub struct TonedRender {
    pub rgba: Vec<u8>,
    pub spans: Vec<ToneSpan>,
}

/// Encodes `bytes` as toned pixels, changing the base colour at the given
/// byte offsets. Each colour is auto-tuned with [`TonedConfig::balanced`]
/// under `max_size_factor` (palettes cached per colour). Spans are encoded
/// independently (each zero-pads its own trailing bits), so any span can be
/// decoded in isolation from its persisted [`ToneSpan`].
pub fn encode_toned_spans(
    bytes: &[u8],
    requests: &[ToneRequest],
    max_size_factor: f64,
) -> Result<TonedRender> {
    if requests.is_empty() {
        bail!("at least one tone request is required");
    }
    if requests[0].byte_offset != 0 {
        bail!("first tone request must start at byte offset 0");
    }
    for pair in requests.windows(2) {
        if pair[1].byte_offset <= pair[0].byte_offset {
            bail!("tone request offsets must be strictly increasing");
        }
    }
    if let Some(last) = requests.last() {
        if last.byte_offset >= bytes.len() && !bytes.is_empty() {
            bail!(
                "tone request offset {} is beyond the payload",
                last.byte_offset
            );
        }
    }

    let mut configs: HashMap<[u8; 3], TonedConfig> = HashMap::new();
    let mut rgba = Vec::new();
    let mut spans = Vec::with_capacity(requests.len());

    for (i, request) in requests.iter().enumerate() {
        let end = requests
            .get(i + 1)
            .map_or(bytes.len(), |next| next.byte_offset);
        let slice = &bytes[request.byte_offset..end];

        let config = match configs.get(&request.base) {
            Some(&config) => config,
            None => {
                let config = TonedConfig::balanced(request.base, max_size_factor)?;
                configs.insert(request.base, config);
                config
            }
        };
        let palette = TonedPalette::shared(config)?;

        let pixel_offset = rgba.len() / 4;
        let span_rgba = palette.bytes_to_rgba(slice);
        spans.push(ToneSpan {
            byte_offset: request.byte_offset,
            byte_length: slice.len(),
            pixel_offset,
            pixel_count: span_rgba.len() / 4,
            config,
        });
        rgba.extend_from_slice(&span_rgba);
    }

    Ok(TonedRender { rgba, spans })
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

/// Decodes pixels produced by [`encode_toned_spans`] using the persisted
/// span settings. Fully transparent pixels (e.g. square-PNG padding) are
/// skipped before spans are applied. Spans are validated for contiguity and
/// exact pixel/byte coverage before decoding, and the result length is checked.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rgba_to_square_png, square_png_to_rgba};

    const PINK: [u8; 3] = [0xff, 0xc0, 0xcb];
    const BLUE: [u8; 3] = [0x20, 0x60, 0xc0];
    const NAVY: [u8; 3] = [0x10, 0x18, 0x40];

    fn payload(len: usize) -> Vec<u8> {
        let mut state = 0xfeed_face_cafe_beefu64;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 56) as u8
            })
            .collect()
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn single_colour_round_trips_exactly() {
        let bytes = payload(4093);
        let render = encode_toned_spans(
            &bytes,
            &[ToneRequest {
                byte_offset: 0,
                base: PINK,
            }],
            1.2,
        )
        .unwrap();
        assert_eq!(render.spans.len(), 1);
        assert_eq!(
            decode_toned_spans(&render.rgba, &render.spans).unwrap(),
            bytes
        );
    }

    #[test]
    #[ignore] // slow: builds large iso-luma palettes; run with --ignored
    fn multi_colour_offsets_round_trip_exactly_including_png() {
        let bytes = payload(10_007);
        let requests = [
            ToneRequest {
                byte_offset: 0,
                base: PINK,
            },
            ToneRequest {
                byte_offset: 3000,
                base: BLUE,
            },
            ToneRequest {
                byte_offset: 3001,
                base: NAVY,
            }, // 1-byte span
            ToneRequest {
                byte_offset: 7500,
                base: PINK,
            }, // palette reused from cache
        ];
        let render = encode_toned_spans(&bytes, &requests, 1.2).unwrap();
        assert_eq!(render.spans.len(), 4);
        assert_eq!(render.spans[0].config, render.spans[3].config);
        assert_eq!(
            render.spans.iter().map(|s| s.byte_length).sum::<usize>(),
            bytes.len()
        );

        // Decode straight, then through square PNG (transparent padding skipped).
        assert_eq!(
            decode_toned_spans(&render.rgba, &render.spans).unwrap(),
            bytes
        );
        let png = rgba_to_square_png(&render.rgba).unwrap();
        let decoded = square_png_to_rgba(&png).unwrap();
        assert_eq!(decode_toned_spans(&decoded, &render.spans).unwrap(), bytes);
    }

    #[test]
    fn invalid_requests_are_rejected() {
        let bytes = payload(100);
        assert!(encode_toned_spans(&bytes, &[], 1.2).is_err());
        assert!(encode_toned_spans(
            &bytes,
            &[ToneRequest {
                byte_offset: 5,
                base: PINK
            }],
            1.2
        )
        .is_err());
        assert!(encode_toned_spans(
            &bytes,
            &[
                ToneRequest {
                    byte_offset: 0,
                    base: PINK
                },
                ToneRequest {
                    byte_offset: 0,
                    base: BLUE
                }
            ],
            1.2
        )
        .is_err());
        assert!(encode_toned_spans(
            &bytes,
            &[
                ToneRequest {
                    byte_offset: 0,
                    base: PINK
                },
                ToneRequest {
                    byte_offset: 200,
                    base: BLUE
                }
            ],
            1.2
        )
        .is_err());
    }
}
