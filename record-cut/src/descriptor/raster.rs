// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Canonical BRD1 metadata raster encoding helpers.

use record_descriptor::{
    metadata_byte_capacity_for_pixel_count, metadata_pixel_count_for_byte_length,
    METADATA_GRAYSCALE_NIBBLE_BASE,
};

pub fn paint_metadata_bytes_as_grayscale(
    data: &mut [u8],
    indices: &[usize],
    bytes: &[u8],
) -> usize {
    let byte_count = bytes
        .len()
        .min(metadata_byte_capacity_for_pixel_count(indices.len()));

    for (byte_number, &byte) in bytes.iter().take(byte_count).enumerate() {
        let high = METADATA_GRAYSCALE_NIBBLE_BASE + ((byte >> 4) & 0x0f);
        let low = METADATA_GRAYSCALE_NIBBLE_BASE + (byte & 0x0f);

        for (nibble_index, value) in [high, low].iter().enumerate() {
            let pixel_index = indices[byte_number * 2 + nibble_index];
            let Some(rgba_index) = pixel_index.checked_mul(4) else {
                continue;
            };
            if rgba_index + 3 >= data.len() {
                continue;
            }

            data[rgba_index] = *value;
            data[rgba_index + 1] = *value;
            data[rgba_index + 2] = *value;
            data[rgba_index + 3] = 255;
        }
    }

    metadata_pixel_count_for_byte_length(byte_count)
}
