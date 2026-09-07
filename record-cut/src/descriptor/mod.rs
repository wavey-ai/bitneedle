// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Canonical BRD1 descriptor construction and metadata raster authoring.
//!
//! This module builds and paints Bitneedle BRD1 descriptors, using the
//! wire-format types and decoder from `record-descriptor`.

pub mod encode;
pub mod raster;

pub use raster::{
    metadata_fade_pixel_count, paint_metadata_bytes_as_grayscale, paint_unused_metadata_groove,
    UNUSED_METADATA_GROOVE_ALPHA, UNUSED_METADATA_GROOVE_FADE_TURNS,
    UNUSED_METADATA_GROOVE_RGB_MIN, UNUSED_METADATA_GROOVE_RGB_SPAN,
};
pub use record_descriptor::{
    grayscale_value_for_level, level_for_grayscale_value, METADATA_GRAYSCALE_BITS_PER_PIXEL,
    METADATA_GRAYSCALE_LEVELS, METADATA_GRAYSCALE_NIBBLE_BASE, METADATA_GRAYSCALE_STEP,
};

pub use encode::{
    encode_record_descriptor_stream, encode_segmented_body, encode_signed_release_reference,
    optional_text, push_segment, RecordDescriptorInput, RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
    RECORD_DESCRIPTOR_TEXT_LIMIT,
};
