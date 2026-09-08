// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! A pressed record, written as something other than a PNG.
//!
//! A record is not a picture of data. Every groove pixel carries payload —
//! three bytes of it in an RGB cut, and one of about a million iso-luma
//! colours in a toned one — so a format that alters a single channel of a
//! single pixel does not degrade the record, it stops the record from
//! decoding. That rules out every lossy format and every palette format:
//! 256 colours cannot hold a palette of 2^20, and a quantizer collapses
//! neighbouring entries first, which is exactly where the payload lives.
//!
//! What is left is the formats that store 8 bits per channel exactly. Each
//! one here is behind its own cargo feature, because a phone that exports
//! PNG has no reason to link a TIFF encoder. PNG is always compiled: it is
//! what the press writes.
//!
//! Alpha is not decoration. The decoder walks the groove until it meets a
//! pixel with zero alpha, which is how it knows where the written groove
//! ends, so a format that drops the alpha channel drops the record. Every
//! format here carries `Rgba8`.

use anyhow::{bail, Context, Result};

/// The raw sidecar's magic and header length, from the crate that reads
/// them. Two copies of a magic number is one copy too many.
#[cfg(feature = "rgba")]
pub use record_decode::{RGBA_SIDECAR_HEADER_LENGTH, RGBA_SIDECAR_MAGIC};

/// A format a record can be written in.
///
/// Every build holds every variant. [`RecordImageFormat::is_available`] reports
/// whether the codec of a variant is compiled in. A build without a codec still
/// knows the name of that format, so one caller lists the formats and another
/// reports the formats that this binary writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordImageFormat {
    /// What the press writes. Deflate, always compiled.
    Png,
    /// Uncompressed TIFF, which is what this encoder writes. The archival
    /// and print format, and the one most other tools open.
    Tiff,
    /// Uncompressed 32-bit BGRA, written with the header that carries an
    /// alpha channel (`BITMAPV4HEADER`). The largest file here.
    Bmp,
    /// The Quite OK Image format: one pass, no entropy coder, a fraction of
    /// PNG's encode time at close to PNG's size.
    Qoi,
    /// VP8L. Lossless is the only WebP this writes.
    WebP,
    /// Truevision TGA, run-length encoded.
    Tga,
    /// The netpbm arbitrary map, `P7`. Written as PAM rather than PPM
    /// because PPM has no alpha channel and a record without its alpha is a
    /// record with no end to its groove.
    Pnm,
    /// No container: the magic, the width, the height, then the pixels. The
    /// reference the others are checked against, and the one format that
    /// cannot lose anything by construction.
    Rgba,
}

impl RecordImageFormat {
    /// Every format, in the order they are worth trying.
    pub const ALL: [Self; 8] = [
        Self::Png,
        Self::Tiff,
        Self::Bmp,
        Self::Qoi,
        Self::WebP,
        Self::Tga,
        Self::Pnm,
        Self::Rgba,
    ];

    /// The name this format is asked for by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Qoi => "qoi",
            Self::WebP => "webp",
            Self::Tga => "tga",
            Self::Pnm => "pnm",
            Self::Rgba => "rgba",
        }
    }

    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Qoi => "qoi",
            Self::WebP => "webp",
            Self::Tga => "tga",
            // `P7` is the arbitrary map, and its extension is `pam`.
            Self::Pnm => "pam",
            Self::Rgba => "rgba",
        }
    }

    pub fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Tiff => "image/tiff",
            Self::Bmp => "image/bmp",
            Self::Qoi => "image/qoi",
            Self::WebP => "image/webp",
            Self::Tga => "image/x-tga",
            Self::Pnm => "image/x-portable-arbitrarymap",
            Self::Rgba => "application/octet-stream",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.id() == id)
    }

    /// Whether this build carries the codec.
    pub fn is_available(self) -> bool {
        match self {
            Self::Png => true,
            Self::Tiff => cfg!(feature = "tiff"),
            Self::Bmp => cfg!(feature = "bmp"),
            Self::Qoi => cfg!(feature = "qoi"),
            Self::WebP => cfg!(feature = "webp"),
            Self::Tga => cfg!(feature = "tga"),
            Self::Pnm => cfg!(feature = "pnm"),
            Self::Rgba => cfg!(feature = "rgba"),
        }
    }

    /// The formats this build can write and read.
    pub fn available() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|format| format.is_available())
            .collect()
    }
}

/// The one shape every codec here is driven through: build the encoder over
/// a growing buffer, hand it the record as `Rgba8`, give back the bytes.
///
/// A macro rather than a function taking a closure: each encoder borrows
/// the buffer for a different lifetime and one of them wants `Seek` as
/// well, which no single closure signature covers.
#[cfg(any(
    feature = "tiff",
    feature = "bmp",
    feature = "qoi",
    feature = "webp",
    feature = "tga",
    feature = "pnm"
))]
macro_rules! encoded {
    ($width:expr, $height:expr, $rgba:expr, $what:literal, |$out:ident| $encoder:expr) => {{
        use image::{ExtendedColorType, ImageEncoder};
        let mut buffer = Vec::new();
        {
            let $out = &mut buffer;
            $encoder
                .write_image($rgba, $width as u32, $height as u32, ExtendedColorType::Rgba8)
                .context(concat!("failed to encode record ", $what))?;
        }
        Ok(buffer)
    }};
}

/// One record's pixels, as one of the formats above.
///
/// `rgba` is `width * height * 4` bytes, straight off the render — the same
/// buffer [`crate::write_rgba_png`] takes, and the same one every decoder
/// here hands back.
pub fn write_rgba(
    format: RecordImageFormat,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Result<Vec<u8>> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("record dimensions overflow")?;
    if rgba.len() != expected {
        bail!(
            "{width}x{height} needs {expected} bytes of RGBA, got {}",
            rgba.len()
        );
    }

    match format {
        RecordImageFormat::Png => crate::write_rgba_png(width, height, rgba),
        RecordImageFormat::Rgba => {
            #[cfg(feature = "rgba")]
            {
                let mut out = Vec::with_capacity(RGBA_SIDECAR_HEADER_LENGTH + rgba.len());
                out.extend_from_slice(RGBA_SIDECAR_MAGIC);
                out.extend_from_slice(&(width as u32).to_le_bytes());
                out.extend_from_slice(&(height as u32).to_le_bytes());
                out.extend_from_slice(rgba);
                Ok(out)
            }
            #[cfg(not(feature = "rgba"))]
            missing(format)
        }
        RecordImageFormat::Tiff => {
            #[cfg(feature = "tiff")]
            {
                // The one encoder that seeks: a TIFF's header holds offsets
                // it can only fill in once it knows where things landed.
                encoded!(width, height, rgba, "TIFF", |out| {
                    image::codecs::tiff::TiffEncoder::new(std::io::Cursor::new(out))
                })
            }
            #[cfg(not(feature = "tiff"))]
            missing(format)
        }
        RecordImageFormat::Bmp => {
            #[cfg(feature = "bmp")]
            {
                encoded!(width, height, rgba, "BMP", |out| {
                    image::codecs::bmp::BmpEncoder::new(out)
                })
            }
            #[cfg(not(feature = "bmp"))]
            missing(format)
        }
        RecordImageFormat::Qoi => {
            #[cfg(feature = "qoi")]
            {
                encoded!(width, height, rgba, "QOI", |out| {
                    image::codecs::qoi::QoiEncoder::new(out)
                })
            }
            #[cfg(not(feature = "qoi"))]
            missing(format)
        }
        RecordImageFormat::WebP => {
            #[cfg(feature = "webp")]
            {
                encoded!(width, height, rgba, "WebP", |out| {
                    image::codecs::webp::WebPEncoder::new_lossless(out)
                })
            }
            #[cfg(not(feature = "webp"))]
            missing(format)
        }
        RecordImageFormat::Tga => {
            #[cfg(feature = "tga")]
            {
                encoded!(width, height, rgba, "TGA", |out| {
                    image::codecs::tga::TgaEncoder::new(out)
                })
            }
            #[cfg(not(feature = "tga"))]
            missing(format)
        }
        RecordImageFormat::Pnm => {
            #[cfg(feature = "pnm")]
            {
                // `ArbitraryMap` is P7, and it is not a preference: the
                // other subtypes have no alpha channel, and the decoder
                // finds the end of the groove by its alpha.
                encoded!(width, height, rgba, "PAM", |out| {
                    image::codecs::pnm::PnmEncoder::new(out)
                        .with_subtype(image::codecs::pnm::PnmSubtype::ArbitraryMap)
                })
            }
            #[cfg(not(feature = "pnm"))]
            missing(format)
        }
    }
}

/// A pressed record read back to pixels, whatever it was written as.
///
/// The reader is `record-decode`'s, which is the one that has to be right:
/// a record is read back by whatever opens it, and an exporter holding a
/// second opinion about what its own files contain is how an export comes
/// to be unreadable by the app that wrote it.
pub fn read_rgba(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    record_decode::load_record_rgba(bytes)
}

/// A record written in one format, written again in another.
///
/// The pixels are not touched on the way through: whatever came in is what
/// goes out, which is the only property that matters about a record.
pub fn transcode(bytes: &[u8], format: RecordImageFormat) -> Result<Vec<u8>> {
    let (width, height, rgba) = read_rgba(bytes)?;
    write_rgba(format, width, height, &rgba)
}

#[allow(dead_code)]
fn missing(format: RecordImageFormat) -> Result<Vec<u8>> {
    bail!(
        "record-render was built without the `{}` feature, so it cannot write {}",
        format.id(),
        format.id()
    )
}
