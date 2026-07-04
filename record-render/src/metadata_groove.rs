// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

pub const UNUSED_METADATA_GROOVE_RGB_MIN: u8 = 112;
pub const UNUSED_METADATA_GROOVE_RGB_SPAN: u8 = 32;
pub const UNUSED_METADATA_GROOVE_ALPHA: u8 = 128;
pub const UNUSED_METADATA_GROOVE_FADE_TURNS: f64 = 0.5;

pub fn metadata_fade_pixel_count(pixel_count: usize, turns: f64) -> usize {
    if pixel_count == 0 {
        return 0;
    }

    let pixels_per_turn = pixel_count as f64 / turns.max(0.01);
    (pixels_per_turn * UNUSED_METADATA_GROOVE_FADE_TURNS)
        .round()
        .max(1.0) as usize
}

pub fn paint_unused_metadata_groove(
    data: &mut [u8],
    indices: &[usize],
    start_pixel: usize,
    salt: usize,
    fade_pixels: usize,
) {
    for (pixel_number, &pixel_index) in indices.iter().enumerate().skip(start_pixel) {
        let Some(rgba_index) = pixel_index.checked_mul(4) else {
            continue;
        };
        if rgba_index + 3 >= data.len() {
            continue;
        }

        let dither = metadata_dither(pixel_index, pixel_number, salt);
        let gray = UNUSED_METADATA_GROOVE_RGB_MIN
            + (dither.rotate_left(1) % (UNUSED_METADATA_GROOVE_RGB_SPAN + 1));
        let alpha = unused_metadata_alpha(pixel_number, start_pixel, fade_pixels);

        data[rgba_index] = gray;
        data[rgba_index + 1] = gray;
        data[rgba_index + 2] = gray;
        data[rgba_index + 3] = alpha;
    }
}

fn unused_metadata_alpha(pixel_number: usize, start_pixel: usize, fade_pixels: usize) -> u8 {
    let offset = pixel_number.saturating_sub(start_pixel);
    if fade_pixels == 0 || offset >= fade_pixels {
        return UNUSED_METADATA_GROOVE_ALPHA;
    }

    let t = ((offset + 1) as f64 / fade_pixels as f64).clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    let alpha = 255.0 - ((255.0 - f64::from(UNUSED_METADATA_GROOVE_ALPHA)) * eased);

    alpha
        .round()
        .clamp(f64::from(UNUSED_METADATA_GROOVE_ALPHA), 255.0) as u8
}

fn metadata_dither(pixel_index: usize, sequence_index: usize, salt: usize) -> u8 {
    let mut value = pixel_index as u64;
    value ^= (sequence_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value ^= (salt as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value & 0xff) as u8
}
