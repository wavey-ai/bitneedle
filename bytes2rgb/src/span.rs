//! Byte-offset tone spans used by the decoder.

use crate::{TonedConfig, TonedPalette};
use anyhow::{bail, Result};

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

/// Decodes toned pixels using the persisted span settings. Fully transparent
/// pixels are skipped before spans are applied.
pub fn decode_toned_spans(rgba: &[u8], spans: &[ToneSpan]) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let opaque: Vec<u8> = rgba
        .chunks_exact(4)
        .filter(|chunk| chunk[3] != 0)
        .flatten()
        .copied()
        .collect();

    let mut bytes = Vec::new();

    for span in spans {
        if span.byte_offset != bytes.len() {
            bail!(
                "span byte offset {} does not follow previous spans ({} bytes decoded)",
                span.byte_offset,
                bytes.len()
            );
        }
        let start = span.pixel_offset * 4;
        let end = start + span.pixel_count * 4;
        if end > opaque.len() {
            bail!("span pixel range exceeds the pixel stream");
        }

        let palette = TonedPalette::shared(span.config)?;
        bytes.extend(palette.rgba_to_bytes(&opaque[start..end], Some(span.byte_length))?);
    }

    Ok(bytes)
}
