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

pub use encode::{
    encode_record_descriptor_stream, encode_segmented_body, encode_signed_release_reference,
    optional_text, push_segment, RecordDescriptorInput, RECORD_DESCRIPTOR_CREATOR_TEXT_LIMIT,
    RECORD_DESCRIPTOR_TEXT_LIMIT,
};
pub use raster::paint_metadata_bytes_as_grayscale;
