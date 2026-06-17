use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static PALETTE_CACHE: OnceLock<Mutex<HashMap<TonedConfig, Arc<TonedPalette>>>> = OnceLock::new();

/// Number of RGB pixels needed to carry `byte_length` bytes at 3 bytes per pixel.
pub fn pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.div_ceil(3)
}

/// Recovers the byte stream packed into the RGB channels of an RGBA buffer,
/// skipping fully transparent pixels. When `byte_length` is given the result
/// is truncated to that length.
pub fn rgba_to_bytes(rgba: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let mut bytes = Vec::with_capacity((rgba.len() / 4) * 3);

    for chunk in rgba.chunks_exact(4) {
        if chunk[3] == 0 {
            continue;
        }

        bytes.extend_from_slice(&chunk[..3]);
    }

    if let Some(byte_length) = byte_length {
        if byte_length > bytes.len() {
            bail!("requested byte length exceeds decoded RGB payload");
        }

        bytes.truncate(byte_length);
    }

    Ok(bytes)
}

/// Recovers the byte stream packed into grayscale RGBA pixels, skipping fully
/// transparent pixels. Fails if an opaque pixel is not grayscale (R = G = B).
/// When `byte_length` is given the result is truncated to that length.
pub fn grayscale_rgba_to_bytes(rgba: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
    if rgba.len() % 4 != 0 {
        bail!("RGBA length must be divisible by 4");
    }

    let mut bytes = Vec::with_capacity(rgba.len() / 4);

    for chunk in rgba.chunks_exact(4) {
        if chunk[3] == 0 {
            continue;
        }

        if chunk[0] != chunk[1] || chunk[1] != chunk[2] {
            bail!("pixel is not grayscale");
        }

        bytes.push(chunk[0]);
    }

    if let Some(byte_length) = byte_length {
        if byte_length > bytes.len() {
            bail!("requested byte length exceeds decoded grayscale payload");
        }

        bytes.truncate(byte_length);
    }

    Ok(bytes)
}

/// Copies the RGB channels of one pixel to another, leaving alpha untouched.
pub fn copy_rgb(
    source: &[u8],
    source_pixel_index: usize,
    target: &mut [u8],
    target_pixel_index: usize,
) {
    let source_offset = source_pixel_index * 4;
    let target_offset = target_pixel_index * 4;
    target[target_offset] = source[source_offset];
    target[target_offset + 1] = source[source_offset + 1];
    target[target_offset + 2] = source[source_offset + 2];
}

/// Rec. 709 luma of a pixel in an RGBA buffer, in the 0.0–255.0 range.
pub fn luma_rec709(data: &[u8], pixel_index: usize) -> f64 {
    let offset = pixel_index * 4;
    0.2126 * data[offset] as f64
        + 0.7152 * data[offset + 1] as f64
        + 0.0722 * data[offset + 2] as f64
}

fn rec709_luma_of(color: [u8; 3]) -> f64 {
    0.2126 * color[0] as f64 + 0.7152 * color[1] as f64 + 0.0722 * color[2] as f64
}

/// Rec. 709 chroma (Cb, Cr) of a colour, signed and centred on 0.
pub fn chroma_rec709(color: [u8; 3]) -> (f64, f64) {
    let luma = rec709_luma_of(color);
    let cb = (color[2] as f64 - luma) / 1.8556;
    let cr = (color[0] as f64 - luma) / 1.5748;
    (cb, cr)
}

fn chroma_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (a_cb, a_cr) = chroma_rec709(a);
    let (b_cb, b_cr) = chroma_rec709(b);
    ((a_cb - b_cb).powi(2) + (a_cr - b_cr).powi(2)).sqrt()
}

/// How palette colours are ordered (and therefore which `2^bits_per_pixel`
/// subset of the iso-luma candidates carries the data). Both orderings are
/// deterministic, so the same config always rebuilds the same palette and the
/// byte stream decodes exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ToneOrdering {
    /// Closest RGB distance to the base tone first.
    #[default]
    BaseProximity,
    /// Closest Rec. 709 chroma (Cb/Cr) distance to the base tone first. This
    /// pulls the palette's average colour toward the base tone's hue instead
    /// of the iso-luma set's centroid (which skews green at high luma).
    ChromaProximity,
}

pub mod span;
pub use span::{decode_toned_spans, ToneSpan};

/// Configuration for a [`TonedPalette`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TonedConfig {
    /// Base tone whose luma every pixel preserves (within `luma_tolerance`).
    pub base: [u8; 3],
    /// Allowed deviation, in rounded Rec. 709 luma steps, around the base
    /// tone's luma. `0` keeps brightness exactly flat; larger values unlock
    /// more colours (capacity) at the cost of brightness drift.
    pub luma_tolerance: u8,
    /// Bits encoded per pixel. The palette must contain at least
    /// `2^bits_per_pixel` iso-luma colours, so this is bounded by
    /// `luma_tolerance`. Higher means smaller output (fewer pixels).
    pub bits_per_pixel: u32,
    /// Which candidate colours make up the palette; see [`ToneOrdering`].
    pub ordering: ToneOrdering,
}

impl TonedConfig {
    /// Configuration from a CSS-style hex base tone (`#RRGGBB`, `#RGB`, or the
    /// same without `#`).
    pub fn from_hex(base: &str, luma_tolerance: u8, bits_per_pixel: u32) -> Self {
        let hex = normalized_hex_color(Some(base));
        let parse =
            |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).unwrap_or(0);
        Self {
            base: [parse(1..3), parse(3..5), parse(5..7)],
            luma_tolerance,
            bits_per_pixel,
            ordering: ToneOrdering::default(),
        }
    }

}

/// A palette of colours sharing (within `luma_tolerance` integer steps) the
/// Rec. 709 luma of a base tone, ordered by closeness to that tone. Data is
/// carried as chroma drift: each pixel encodes `bits_per_pixel` bits as an
/// index into the palette, so brightness stays flat while hue/saturation
/// wander only as far as the requested capacity demands.
pub struct TonedPalette {
    config: TonedConfig,
    colors: Vec<[u8; 3]>,
    index_of: HashMap<[u8; 3], u32>,
}

impl TonedPalette {
    /// Every 8-bit RGB colour whose rounded luma is within `luma_tolerance`
    /// of the base tone's, in enumeration order (unsorted).
    fn enumerate_iso_luma(base: [u8; 3], luma_tolerance: u8) -> Vec<[u8; 3]> {
        let target = rec709_luma_of(base).round();
        let min = target - luma_tolerance as f64;
        let max = target + luma_tolerance as f64;

        let mut colors = Vec::new();
        for r in 0..=255u16 {
            for g in 0..=255u16 {
                let rg = 0.2126 * r as f64 + 0.7152 * g as f64;
                // round(rg + 0.0722 * b) must land in [min, max]
                let b_low = ((min - 0.5 - rg) / 0.0722).floor().max(0.0) as u16;
                let b_high = ((max + 0.5 - rg) / 0.0722).ceil().min(255.0) as u16;
                for b in b_low..=b_high.min(255) {
                    let luma = (rg + 0.0722 * b as f64).round();
                    if luma >= min && luma <= max {
                        colors.push([r as u8, g as u8, b as u8]);
                    }
                }
            }
        }
        colors
    }

    /// Process-wide cached palette for `config`. Palettes are deterministic
    /// and immutable, so encode, decode and metadata layers share one build
    /// per config instead of recomputing it.
    pub fn shared(config: TonedConfig) -> Result<Arc<Self>> {
        let cache = PALETTE_CACHE.get_or_init(Default::default);
        if let Some(palette) = cache.lock().unwrap().get(&config) {
            return Ok(palette.clone());
        }
        let palette = Arc::new(Self::from_config(config)?);
        cache.lock().unwrap().insert(config, palette.clone());
        Ok(palette)
    }

    pub fn from_config(config: TonedConfig) -> Result<Self> {
        let TonedConfig {
            base,
            luma_tolerance,
            bits_per_pixel,
            ordering,
        } = config;

        if !(1..=24).contains(&bits_per_pixel) {
            bail!("bits per pixel must be between 1 and 24");
        }

        // Key every candidate once, partition the needed prefix with
        // select_nth (O(n)) and only sort that prefix — identical result to
        // a full sort + truncate, several times faster at palette sizes.
        let key = |c: [u8; 3]| -> f64 {
            match ordering {
                ToneOrdering::ChromaProximity => chroma_distance(c, base),
                ToneOrdering::BaseProximity => c
                    .iter()
                    .zip(base.iter())
                    .map(|(&a, &b)| ((a as i32 - b as i32).pow(2)) as f64)
                    .sum(),
            }
        };
        let mut keyed: Vec<(f64, [u8; 3])> = Self::enumerate_iso_luma(base, luma_tolerance)
            .into_iter()
            .map(|c| (key(c), c))
            .collect();

        let needed = 1usize << bits_per_pixel;
        if keyed.len() < needed {
            bail!(
                "only {} colours share the base tone's luma (±{}); {} bits per pixel needs {}",
                keyed.len(),
                luma_tolerance,
                bits_per_pixel,
                needed
            );
        }
        let compare =
            |a: &(f64, [u8; 3]), b: &(f64, [u8; 3])| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
        if keyed.len() > needed {
            keyed.select_nth_unstable_by(needed - 1, compare);
            keyed.truncate(needed);
        }
        keyed.sort_unstable_by(compare);
        let colors: Vec<[u8; 3]> = keyed.into_iter().map(|(_, c)| c).collect();

        let index_of = colors
            .iter()
            .enumerate()
            .map(|(index, &color)| (color, index as u32))
            .collect();

        Ok(Self {
            config,
            colors,
            index_of,
        })
    }

    /// The configuration this palette was built from. Rebuilding from it is
    /// deterministic, so it is all a decoder needs.
    pub fn config(&self) -> TonedConfig {
        self.config
    }

    /// Number of colours in the palette (`2^bits_per_pixel`).
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Recovers the byte stream from toned pixels, skipping fully transparent
    /// pixels. When `byte_length` is given the result is truncated to that
    /// length.
    pub fn rgba_to_bytes(&self, rgba: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
        if rgba.len() % 4 != 0 {
            bail!("RGBA length must be divisible by 4");
        }

        let bpp = self.config.bits_per_pixel;
        let mut bytes = Vec::with_capacity(rgba.len() / 4 * bpp as usize / 8 + 1);
        let mut acc = 0u32;
        let mut acc_bits = 0u32;

        for chunk in rgba.chunks_exact(4) {
            if chunk[3] == 0 {
                continue;
            }

            let Some(&index) = self.index_of.get(&[chunk[0], chunk[1], chunk[2]]) else {
                bail!(
                    "pixel #{:02X}{:02X}{:02X} is not in the toned palette",
                    chunk[0],
                    chunk[1],
                    chunk[2]
                );
            };

            acc = (acc << bpp) | index;
            acc_bits += bpp;
            while acc_bits >= 8 {
                bytes.push((acc >> (acc_bits - 8)) as u8);
                acc_bits -= 8;
                acc &= (1 << acc_bits) - 1;
            }
        }

        if let Some(byte_length) = byte_length {
            if byte_length > bytes.len() {
                bail!("requested byte length exceeds decoded toned payload");
            }
            bytes.truncate(byte_length);
        }

        Ok(bytes)
    }
}

/// Side length of the smallest square that holds `pixel_count` pixels.
pub fn square_side_for_pixel_count(pixel_count: usize) -> usize {
    let mut side = (pixel_count as f64).sqrt() as usize;
    while side * side < pixel_count {
        side += 1;
    }
    side
}

/// Decodes an 8-bit RGBA PNG into its raw RGBA buffer.
pub fn square_png_to_rgba(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let decoder = png::Decoder::new(png_bytes);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer)?;

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        bail!("expected an 8-bit RGBA PNG");
    }

    buffer.truncate(info.buffer_size());
    Ok(buffer)
}

/// Normalizes a CSS-style hex colour (with or without `#`, 3 or 6 digits) to
/// uppercase `#RRGGBB`, falling back to white on invalid input.
pub fn normalized_hex_color(value: Option<&str>) -> String {
    let raw = value.unwrap_or("").trim().trim_start_matches('#');
    let expanded = if raw.len() == 3 {
        raw.chars().flat_map(|ch| [ch, ch]).collect::<String>()
    } else {
        raw.to_string()
    };
    let parsed = u32::from_str_radix(&expanded, 16).unwrap_or(0xff_ffff);
    format!(
        "#{:02X}{:02X}{:02X}",
        (parsed >> 16) & 0xff,
        (parsed >> 8) & 0xff,
        parsed & 0xff
    )
}
