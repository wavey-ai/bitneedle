use anyhow::{bail, Context, Result};
use bytes2rgb::{copy_rgb, rgba_to_bytes as track_rgba_to_bytes};
use record_core::{
    build_header_spiral_indices, build_spiral_mask, build_trailer_spiral_indices,
    known_record_profile_names, normalize_record_profile_name,
};
use record_descriptor::RecordDescriptor;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternReverseMode {
    FullSort,
    PartialSort,
    PairSwap,
    DeltaCarrier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternReverseEntryKind {
    Sort,
    PairSwap,
}

#[derive(Debug, Clone)]
struct PatternItem {
    pixel_index: usize,
    source_rank: usize,
    block_offset: usize,
}

#[derive(Debug, Clone)]
struct PatternReverseMapHeader {
    mode: PatternReverseMode,
    layout_id: u32,
    block_size: usize,
    amount: f64,
    width: usize,
    height: usize,
    groove_pixel_count: usize,
    source_stream_byte_length: usize,
}

#[derive(Debug, Clone)]
struct PatternReverseMapEntry {
    kind: PatternReverseEntryKind,
    block_index: usize,
    block_len: usize,
    item_count: usize,
    selected_offsets: Option<Vec<usize>>,
    encoded: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PatternReverseMap {
    header: PatternReverseMapHeader,
    entries: Vec<PatternReverseMapEntry>,
}

#[derive(Debug, Clone, Copy)]
struct SidecarCarrierRegions {
    label: bool,
    payload_intergroove: bool,
    lead_in_deadwax: bool,
}

#[derive(Debug, Clone, Copy)]
struct DummySpiralPixelRegion {
    carrier_pixel_start: usize,
    pixel_count: usize,
}

const PATTERN_REVERSE_MAP_MAGIC: &[u8; 4] = b"BNPM";
const PATTERN_REVERSE_MAP_VERSION: u8 = 1;
const PATTERN_REVERSE_MAP_HEADER_LENGTH: usize = 64;
const PATTERN_REVERSE_MAP_ENTRY_SORT: u8 = 1;
const PATTERN_REVERSE_MAP_ENTRY_PAIR_SWAP: u8 = 2;
const PATTERN_REVERSE_MAP_FLAG_SELECTED_OFFSETS: u8 = 0x01;
const PATTERN_REVERSE_MAP_MIME: &str = "application/vnd.bitneedle.pattern-map";
const PATTERN_REVERSE_MAP_NAME: &str = "bitneedle-pattern-map";

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

    let payload_len = u16::from_be_bytes(prefix[5..7].try_into().expect("slice length")) as usize;

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

fn json_usize_field(value: &serde_json::Value, names: &[&str]) -> Option<usize> {
    names.iter().find_map(|name| {
        let raw = value.get(*name)?;
        raw.as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .or_else(|| {
                raw.as_f64().and_then(|number| {
                    if number.is_finite() && number >= 0.0 {
                        Some(number.floor() as usize)
                    } else {
                        None
                    }
                })
            })
    })
}

fn dummy_spiral_pixel_regions_from_descriptor(
    descriptor: &RecordDescriptor,
) -> Vec<DummySpiralPixelRegion> {
    let Some(raw) = descriptor.arbitrary_metadata.as_deref() else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(regions) = metadata
        .get("dummySpiralRegions")
        .or_else(|| metadata.get("dummy_spiral_regions"))
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };

    let mut parsed = regions
        .iter()
        .filter_map(|region| {
            let carrier_pixel_start =
                json_usize_field(region, &["carrierPixelStart", "spiralPixelStart"])?;
            let pixel_count = json_usize_field(region, &["pixelCount", "spiralPixelCount"])?;
            if pixel_count == 0 {
                return None;
            }
            Some(DummySpiralPixelRegion {
                carrier_pixel_start,
                pixel_count,
            })
        })
        .collect::<Vec<_>>();

    parsed.sort_by_key(|region| region.carrier_pixel_start);
    parsed
}

/// Read the label-text avoid boxes (+ feather + seed) the encoder stored in the
/// record descriptor's arbitrary metadata, so the groove-permutation mask can be
/// reproduced byte-identically. Mirrors the encode side (record-wasm options
/// `textAvoidBoxes`/`textAvoidSeed`/`textAvoidFeatherPx`).
fn text_avoid_mask_params_from_descriptor(
    descriptor: &RecordDescriptor,
) -> (
    Vec<record_core::TextAvoidBox>,
    record_core::TextAvoidMaskParams,
) {
    let defaults = record_core::TextAvoidMaskParams::default();
    let empty = (Vec::new(), defaults);
    let Some(raw) = descriptor.arbitrary_metadata.as_deref() else {
        return empty;
    };
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(raw) else {
        return empty;
    };
    let Some(boxes_value) = metadata
        .get("textAvoidBoxes")
        .or_else(|| metadata.get("text_avoid_boxes"))
    else {
        return empty;
    };
    let boxes: Vec<record_core::TextAvoidBox> =
        serde_json::from_value(boxes_value.clone()).unwrap_or_default();
    if boxes.is_empty() {
        return empty;
    }
    let params = record_core::TextAvoidMaskParams {
        feather_px: metadata
            .get("textAvoidFeatherPx")
            .or_else(|| metadata.get("text_avoid_feather_px"))
            .and_then(|value| value.as_f64())
            .unwrap_or(defaults.feather_px),
        seed: metadata
            .get("textAvoidSeed")
            .or_else(|| metadata.get("text_avoid_seed"))
            .and_then(|value| value.as_u64())
            .unwrap_or(u64::from(defaults.seed)) as u32,
        interior_protect: metadata
            .get("textAvoidInnerProtect")
            .or_else(|| metadata.get("text_avoid_inner_protect"))
            .and_then(|value| value.as_f64())
            .unwrap_or(defaults.interior_protect),
        feather_protect: metadata
            .get("textAvoidFeatherProtect")
            .or_else(|| metadata.get("text_avoid_feather_protect"))
            .and_then(|value| value.as_f64())
            .unwrap_or(defaults.feather_protect),
    };
    (boxes, params)
}

fn decode_record_groove_to_track_data(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    b_value: f64,
    dummy_spiral_regions: &[DummySpiralPixelRegion],
) -> Result<(Vec<u8>, usize)> {
    if rgba.len() != width * height * 4 {
        bail!("record RGBA length does not match width * height * 4");
    }

    let mask = build_spiral_mask(width, height, b_value, record_profile, None, None, None)?;
    let mut track_data = Vec::with_capacity(mask.ordered_pixel_indices.len() * 4);
    let mut carrier_pixels_read = 0usize;
    let mut dummy_region_index = 0usize;
    let mut dummy_region_pixels_read = 0usize;

    for &pixel_index in &mask.ordered_pixel_indices {
        let rgba_index = pixel_index * 4;
        let alpha = rgba[rgba_index + 3];

        while let Some(region) = dummy_spiral_regions.get(dummy_region_index) {
            if carrier_pixels_read <= region.carrier_pixel_start {
                break;
            }
            dummy_region_index += 1;
            dummy_region_pixels_read = 0;
        }

        if let Some(region) = dummy_spiral_regions.get(dummy_region_index) {
            if carrier_pixels_read >= region.carrier_pixel_start
                && dummy_region_pixels_read < region.pixel_count
            {
                if alpha == 0 {
                    break;
                }
                dummy_region_pixels_read += 1;
                if dummy_region_pixels_read >= region.pixel_count {
                    dummy_region_index += 1;
                    dummy_region_pixels_read = 0;
                }
                continue;
            }
        }

        if alpha == 0 {
            break;
        }

        track_data.extend_from_slice(&rgba[rgba_index..rgba_index + 4]);
        carrier_pixels_read += 1;
    }

    if track_data.is_empty() {
        bail!("groove-ordered decode found no written groove pixels");
    }

    let pixel_count = track_data.len() / 4;

    Ok((track_data, pixel_count))
}

fn read_u16_be_at(bytes: &[u8], offset: usize, label: &str) -> Result<usize> {
    let end = offset
        .checked_add(2)
        .with_context(|| format!("{label} offset overflow"))?;
    if end > bytes.len() {
        bail!("{label} is truncated");
    }
    Ok(u16::from_be_bytes(bytes[offset..end].try_into().expect("slice length")) as usize)
}

fn read_u32_be_at(bytes: &[u8], offset: usize, label: &str) -> Result<usize> {
    let end = offset
        .checked_add(4)
        .with_context(|| format!("{label} offset overflow"))?;
    if end > bytes.len() {
        bail!("{label} is truncated");
    }
    Ok(u32::from_be_bytes(bytes[offset..end].try_into().expect("slice length")) as usize)
}

fn parse_pattern_reverse_mode(code: u8) -> Result<PatternReverseMode> {
    match code {
        0 => Ok(PatternReverseMode::FullSort),
        1 => Ok(PatternReverseMode::PartialSort),
        2 => Ok(PatternReverseMode::PairSwap),
        3 => Ok(PatternReverseMode::DeltaCarrier),
        _ => bail!("unsupported pattern reverse map mode {code}"),
    }
}

fn parse_pattern_reverse_entry_kind(code: u8) -> Result<PatternReverseEntryKind> {
    match code {
        PATTERN_REVERSE_MAP_ENTRY_SORT => Ok(PatternReverseEntryKind::Sort),
        PATTERN_REVERSE_MAP_ENTRY_PAIR_SWAP => Ok(PatternReverseEntryKind::PairSwap),
        _ => bail!("unsupported pattern reverse map entry type {code}"),
    }
}

fn pattern_entry_body_len(kind: PatternReverseEntryKind, item_count: usize) -> usize {
    match kind {
        PatternReverseEntryKind::Sort => permutation_byte_length(item_count),
        PatternReverseEntryKind::PairSwap => (item_count / 2).div_ceil(8),
    }
}

fn parse_pattern_reverse_map(bytes: &[u8]) -> Result<PatternReverseMap> {
    if bytes.len() < PATTERN_REVERSE_MAP_HEADER_LENGTH {
        bail!("pattern reverse map is truncated");
    }
    if &bytes[..4] != PATTERN_REVERSE_MAP_MAGIC {
        bail!("pattern reverse map magic mismatch");
    }
    if bytes[4] != PATTERN_REVERSE_MAP_VERSION {
        bail!("unsupported pattern reverse map version {}", bytes[4]);
    }

    let mode = parse_pattern_reverse_mode(bytes[5])?;
    let layout_id = read_u16_be_at(bytes, 6, "pattern layout id")? as u32;
    let block_size = read_u16_be_at(bytes, 8, "pattern block size")?;
    if block_size < 2 {
        bail!("pattern block size must be at least 2");
    }
    let amount = read_u16_be_at(bytes, 10, "pattern amount")? as f64 / 10_000.0;
    let header_len = read_u16_be_at(bytes, 12, "pattern header length")?;
    if header_len != PATTERN_REVERSE_MAP_HEADER_LENGTH {
        bail!("unsupported pattern reverse map header length {header_len}");
    }
    if header_len > bytes.len() {
        bail!("pattern reverse map header exceeds map length");
    }

    let width = read_u16_be_at(bytes, 14, "pattern width")?;
    let height = read_u16_be_at(bytes, 16, "pattern height")?;
    let entry_count = read_u32_be_at(bytes, 18, "pattern entry count")?;
    let groove_pixel_count = read_u32_be_at(bytes, 22, "pattern groove pixel count")?;
    let data_len = read_u32_be_at(bytes, 38, "pattern data length")?;
    let source_stream_byte_length = read_u32_be_at(bytes, 42, "pattern source stream byte length")?;

    if header_len
        .checked_add(data_len)
        .context("pattern reverse map length overflow")?
        != bytes.len()
    {
        bail!("pattern reverse map data length does not match map length");
    }

    let mut entries = Vec::with_capacity(entry_count);
    let mut offset = header_len;
    for _ in 0..entry_count {
        let entry_header_end = offset
            .checked_add(10)
            .context("pattern reverse map entry offset overflow")?;
        if entry_header_end > bytes.len() {
            bail!("pattern reverse map entry header is truncated");
        }

        let block_index = read_u32_be_at(bytes, offset, "pattern block index")?;
        let block_len = read_u16_be_at(bytes, offset + 4, "pattern block length")?;
        let item_count = read_u16_be_at(bytes, offset + 6, "pattern item count")?;
        let kind = parse_pattern_reverse_entry_kind(bytes[offset + 8])?;
        let flags = bytes[offset + 9];
        if flags & !PATTERN_REVERSE_MAP_FLAG_SELECTED_OFFSETS != 0 {
            bail!("pattern reverse map entry has unsupported flags {flags:#04x}");
        }
        if item_count > block_len {
            bail!("pattern reverse map item count exceeds block length");
        }
        offset = entry_header_end;

        let selected_offsets = if flags & PATTERN_REVERSE_MAP_FLAG_SELECTED_OFFSETS != 0 {
            let offsets_len = item_count
                .checked_mul(2)
                .context("pattern selected offsets length overflow")?;
            let offsets_end = offset
                .checked_add(offsets_len)
                .context("pattern selected offsets offset overflow")?;
            if offsets_end > bytes.len() {
                bail!("pattern selected offsets are truncated");
            }
            let mut offsets = Vec::with_capacity(item_count);
            for index in 0..item_count {
                let block_offset =
                    read_u16_be_at(bytes, offset + index * 2, "pattern selected block offset")?;
                if block_offset >= block_len {
                    bail!("pattern selected block offset exceeds block length");
                }
                offsets.push(block_offset);
            }
            offset = offsets_end;
            Some(offsets)
        } else {
            None
        };

        let body_len = pattern_entry_body_len(kind, item_count);
        let body_end = offset
            .checked_add(body_len)
            .context("pattern reverse map entry body offset overflow")?;
        if body_end > bytes.len() {
            bail!("pattern reverse map entry body is truncated");
        }
        let encoded = bytes[offset..body_end].to_vec();
        offset = body_end;

        entries.push(PatternReverseMapEntry {
            kind,
            block_index,
            block_len,
            item_count,
            selected_offsets,
            encoded,
        });
    }

    if offset != bytes.len() {
        bail!("pattern reverse map has trailing bytes");
    }

    Ok(PatternReverseMap {
        header: PatternReverseMapHeader {
            mode,
            layout_id,
            block_size,
            amount,
            width,
            height,
            groove_pixel_count,
            source_stream_byte_length,
        },
        entries,
    })
}

fn permutation_byte_length(count: usize) -> usize {
    let bits = (2..=count).fold(0.0, |total, value| total + (value as f64).log2());
    (bits / 8.0).ceil() as usize
}

fn divide_big_endian_in_place(number: &mut [u8], divisor: usize) -> Result<usize> {
    if divisor == 0 {
        bail!("cannot divide pattern permutation by zero");
    }

    let mut carry = 0usize;
    for byte in number {
        let value = carry
            .checked_mul(256)
            .and_then(|value| value.checked_add(*byte as usize))
            .context("pattern permutation division overflow")?;
        *byte = (value / divisor) as u8;
        carry = value % divisor;
    }
    Ok(carry)
}

fn decode_permutation_factoradic(encoded: &[u8], count: usize) -> Result<Vec<usize>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if count == 1 {
        if !encoded.is_empty() {
            bail!("single-item pattern permutation should be empty");
        }
        return Ok(vec![0]);
    }

    let expected_len = permutation_byte_length(count);
    if encoded.len() != expected_len {
        bail!(
            "pattern permutation length {} does not match expected {}",
            encoded.len(),
            expected_len
        );
    }

    let mut number = encoded.to_vec();
    let mut positions_reversed = Vec::with_capacity(count);
    for base in 1..=count {
        positions_reversed.push(divide_big_endian_in_place(&mut number, base)?);
    }
    if number.iter().any(|byte| *byte != 0) {
        bail!("pattern permutation has bytes outside the expected range");
    }
    positions_reversed.reverse();

    let mut available = (0..count).collect::<Vec<_>>();
    let mut permutation = Vec::with_capacity(count);
    for position in positions_reversed {
        if position >= available.len() {
            bail!("pattern permutation position exceeds available item count");
        }
        permutation.push(available.remove(position));
    }
    Ok(permutation)
}

fn sidecar_label_inner_radius(geometry: &record_core::RecordProfileGeometry) -> i32 {
    record_core::label_sidecar_inner_radius_from_profile_geometry(geometry)
}

fn sidecar_label_outer_radius(geometry: &record_core::RecordProfileGeometry) -> i32 {
    record_core::label_sidecar_outer_radius_from_profile_geometry(geometry)
}

fn sidecar_lead_in_outer_radius(geometry: &record_core::RecordProfileGeometry) -> i32 {
    (geometry.outer_radius - record_core::HEADER_SPIRAL_OUTER_EDGE_INSET)
        .max(geometry.payload_outer_radius + 1)
}

fn build_sidecar_protected_metadata_pixels(
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<Vec<bool>> {
    let mut protected = vec![false; width * height];
    for pixel_index in
        record_core::build_header_spiral_indices(width, height, record_profile, None, None, None)?
    {
        protected[pixel_index] = true;
    }
    for pixel_index in
        record_core::build_trailer_spiral_indices(width, height, record_profile, None, None, None)?
    {
        protected[pixel_index] = true;
    }
    Ok(protected)
}

fn sidecar_pixel_in_carrier_regions(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    geometry: &record_core::RecordProfileGeometry,
    payload_intergroove_is_available: bool,
    regions: SidecarCarrierRegions,
) -> bool {
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let dx = x as f64 + 0.5 - center_x;
    let dy = y as f64 + 0.5 - center_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let label_outer_radius = sidecar_label_outer_radius(geometry) as f64;
    let in_label = regions.label
        && distance > sidecar_label_inner_radius(geometry) as f64
        && distance < label_outer_radius;
    let in_intergroove = regions.payload_intergroove
        && distance > geometry.payload_inner_radius as f64
        && distance < geometry.payload_outer_radius as f64
        && payload_intergroove_is_available;
    let in_lead_in = regions.lead_in_deadwax
        && payload_intergroove_is_available
        && distance > geometry.payload_outer_radius as f64
        && distance < sidecar_lead_in_outer_radius(geometry) as f64;
    let in_deadwax = regions.lead_in_deadwax
        && payload_intergroove_is_available
        && distance > label_outer_radius
        && distance < geometry.payload_inner_radius as f64;
    in_label || in_intergroove || in_lead_in || in_deadwax
}

fn build_sidecar_carrier_region_pairs(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
    regions: SidecarCarrierRegions,
    seed: u32,
    text_avoid: Option<(
        &[record_core::TextAvoidBox],
        &record_core::TextAvoidMaskParams,
    )>,
) -> Result<Vec<(usize, usize)>> {
    let geometry = record_core::describe_record_profile(record_profile)?;
    let mask = if regions.payload_intergroove || regions.lead_in_deadwax {
        Some(record_core::build_spiral_mask(
            width,
            height,
            b_value,
            &geometry.record_profile,
            None,
            None,
            None,
        )?)
    } else {
        None
    };
    let mut protected =
        build_sidecar_protected_metadata_pixels(width, height, &geometry.record_profile)?;
    // Must mirror the encode side (record-wasm build_sidecar_carrier_region_pairs):
    // label-text avoid boxes are excluded from the carrier pairs, so the decode
    // pair list diverges (and the BTS1 magic check fails) unless the same mask is
    // applied here from the descriptor metadata.
    if let Some((boxes, params)) = text_avoid {
        record_core::apply_text_avoid_boxes(&mut protected, width, height, boxes, params);
    }
    let mut pairs = Vec::new();

    for y in 0..height {
        let mut x = 0usize;
        while x + 1 < width {
            let first = y * width + x;
            let second = first + 1;
            if protected[first] || protected[second] {
                x += 1;
                continue;
            }

            let first_payload_intergroove_available = mask
                .as_ref()
                .map(|mask| mask.kinds[first] == 0)
                .unwrap_or(false);
            let second_payload_intergroove_available = mask
                .as_ref()
                .map(|mask| mask.kinds[second] == 0)
                .unwrap_or(false);

            let first_ok = sidecar_pixel_in_carrier_regions(
                x,
                y,
                width,
                height,
                &geometry,
                first_payload_intergroove_available,
                regions,
            );
            let second_ok = sidecar_pixel_in_carrier_regions(
                x + 1,
                y,
                width,
                height,
                &geometry,
                second_payload_intergroove_available,
                regions,
            );

            if first_ok && second_ok {
                pairs.push((first, second));
                x += 2;
            } else {
                x += 1;
            }
        }
    }

    record_sidecar::shuffle_pairs_mulberry32(&mut pairs, seed);
    Ok(pairs)
}

fn build_sidecar_carrier_pairs(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
    carriers: &[record_sidecar::SidecarCarrier],
    seed: u32,
    text_avoid: Option<(
        &[record_core::TextAvoidBox],
        &record_core::TextAvoidMaskParams,
    )>,
) -> Result<Vec<(usize, usize)>> {
    build_sidecar_carrier_region_pairs(
        width,
        height,
        b_value,
        record_profile,
        SidecarCarrierRegions {
            label: carriers.contains(&record_sidecar::SidecarCarrier::Label),
            payload_intergroove: carriers.contains(&record_sidecar::SidecarCarrier::Intergroove),
            lead_in_deadwax: carriers.contains(&record_sidecar::SidecarCarrier::LeadInDeadwax),
        },
        seed,
        text_avoid,
    )
}

fn sidecar_pointer_from_descriptor(
    descriptor: &record_descriptor::RecordDescriptor,
) -> Result<Option<record_sidecar::SidecarHeaderPointer>> {
    let Some(raw) = descriptor.stego_sidecar_descriptor.as_deref() else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let bytes = record_sidecar::decode_base64_text(raw, "steganographic sidecar descriptor")?;
    record_sidecar::decode_sidecar_header_pointer(&bytes).map(Some)
}

fn decode_pattern_reverse_map_from_sidecar(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    descriptor: &RecordDescriptor,
) -> Result<Option<Vec<u8>>> {
    let Some(pointer) = sidecar_pointer_from_descriptor(descriptor)? else {
        return Ok(None);
    };

    let (text_avoid_boxes, text_avoid_params) = text_avoid_mask_params_from_descriptor(descriptor);
    let carrier_pairs = build_sidecar_carrier_pairs(
        width,
        height,
        descriptor.b_value,
        record_profile,
        &pointer.carriers,
        pointer.seed,
        (!text_avoid_boxes.is_empty()).then_some((text_avoid_boxes.as_slice(), &text_avoid_params)),
    )?;
    let capacity_bytes =
        record_sidecar::sidecar_capacity_bytes_for_scheme(&pointer.scheme, carrier_pairs.len())?;
    if pointer.length > capacity_bytes {
        bail!(
            "record sidecar length {} exceeds carrier capacity {}",
            pointer.length,
            capacity_bytes
        );
    }

    let (bts1, _) = record_sidecar::decode_sidecar_from_pairs(
        rgba,
        &carrier_pairs,
        &pointer.scheme,
        pointer.length,
    )
    .with_context(|| {
        format!(
            "failed to decode pattern reverse map sidecar with {} carrier pairs",
            carrier_pairs.len()
        )
    })?;
    let actual = record_sidecar::sha256_base64url(&bts1);
    if actual != pointer.sha256 {
        bail!("record sidecar SHA-256 does not match header pointer");
    }

    let decoded_items = record_sidecar::decode_sidecar_container_items(&bts1)
        .context("decoded pattern sidecar container is invalid")?;
    let Some(decoded_item) = decoded_items.items.iter().find(|item| {
        item.name == PATTERN_REVERSE_MAP_NAME || item.mime == PATTERN_REVERSE_MAP_MIME
    }) else {
        return Ok(None);
    };

    record_sidecar::decode_base64_text(&decoded_item.data_base64, "decoded pattern map").map(Some)
}

fn pattern_selected_groove_indices(
    width: usize,
    height: usize,
    record_profile: &str,
    descriptor: &RecordDescriptor,
    header: &PatternReverseMapHeader,
) -> Result<Vec<usize>> {
    let mask = build_spiral_mask(
        width,
        height,
        descriptor.b_value,
        record_profile,
        None,
        None,
        None,
    )?;
    if header.groove_pixel_count > mask.ordered_pixel_indices.len() {
        bail!("pattern reverse map groove pixel count exceeds available groove pixels");
    }
    let mut protected = build_sidecar_protected_metadata_pixels(width, height, record_profile)?;
    // Reproduce the encode-side label-text protection (tight box + seed-jittered
    // feather) for the groove permutation domain. The sidecar carrier pairs apply
    // the same mask in build_sidecar_carrier_region_pairs.
    let (boxes, params) = text_avoid_mask_params_from_descriptor(descriptor);
    if !boxes.is_empty() {
        record_core::apply_text_avoid_boxes(&mut protected, width, height, &boxes, &params);
    }
    Ok(mask
        .ordered_pixel_indices
        .iter()
        .copied()
        .take(header.groove_pixel_count)
        .filter(|pixel_index| !protected[*pixel_index])
        .collect())
}

fn block_hash_unit(block_index: u64, layout_id: u64) -> f64 {
    let mut value = block_index
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(layout_id.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(0x94d0_49bb_1331_11eb);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value as f64) / (u64::MAX as f64)
}

fn item_hash_unit(block_index: usize, source_rank: usize, layout_id: u32) -> f64 {
    let mixed_block = (block_index as u64).wrapping_mul(0xd6e8_feb8_6659_fd93)
        ^ (source_rank as u64).wrapping_mul(0xa076_1d64_78bd_642f);
    block_hash_unit(mixed_block, layout_id as u64)
}

fn rerank_items(items: Vec<PatternItem>) -> Vec<PatternItem> {
    items
        .into_iter()
        .enumerate()
        .map(|(source_rank, item)| PatternItem {
            pixel_index: item.pixel_index,
            source_rank,
            block_offset: item.block_offset,
        })
        .collect()
}

fn select_full_sort_items(
    block: &[usize],
    block_index: usize,
    layout_id: u32,
    amount: f64,
) -> Vec<PatternItem> {
    let mut items = block
        .iter()
        .copied()
        .enumerate()
        .map(|(block_offset, pixel_index)| PatternItem {
            pixel_index,
            source_rank: block_offset,
            block_offset,
        })
        .collect::<Vec<_>>();

    if items.len() <= 2 || amount >= 0.999 {
        return rerank_items(items);
    }

    let desired = ((items.len() as f64 * amount).ceil() as usize).clamp(2, items.len());
    items.sort_by(|a, b| {
        item_hash_unit(block_index, a.source_rank, layout_id)
            .total_cmp(&item_hash_unit(block_index, b.source_rank, layout_id))
            .then_with(|| a.source_rank.cmp(&b.source_rank))
    });
    let mut selected = items.into_iter().take(desired).collect::<Vec<_>>();
    selected.sort_by_key(|item| item.source_rank);
    rerank_items(selected)
}

fn pattern_items_for_entry(
    header: &PatternReverseMapHeader,
    entry: &PatternReverseMapEntry,
    block: &[usize],
) -> Result<Vec<PatternItem>> {
    if block.len() != entry.block_len {
        bail!("pattern reverse map block length does not match decoded groove block");
    }

    if let Some(offsets) = entry.selected_offsets.as_ref() {
        if offsets.len() != entry.item_count {
            bail!("pattern selected offset count does not match item count");
        }
        return offsets
            .iter()
            .copied()
            .enumerate()
            .map(|(source_rank, block_offset)| {
                let Some(&pixel_index) = block.get(block_offset) else {
                    bail!("pattern selected offset exceeds decoded groove block");
                };
                Ok(PatternItem {
                    pixel_index,
                    source_rank,
                    block_offset,
                })
            })
            .collect();
    }

    if entry.item_count == block.len() {
        return Ok(block
            .iter()
            .copied()
            .enumerate()
            .map(|(source_rank, pixel_index)| PatternItem {
                pixel_index,
                source_rank,
                block_offset: source_rank,
            })
            .collect());
    }

    if header.mode == PatternReverseMode::FullSort {
        let items =
            select_full_sort_items(block, entry.block_index, header.layout_id, header.amount);
        if items.len() != entry.item_count {
            bail!("reconstructed full-sort item count does not match pattern map");
        }
        return Ok(items);
    }

    bail!("pattern map omitted selected offsets for a partial item set")
}

fn positive_modulo(value: i64, divisor: i64) -> i64 {
    if divisor == 0 {
        0
    } else {
        value.rem_euclid(divisor)
    }
}

fn destination_key(item: &PatternItem, count: usize, layout_id: u32) -> [f64; 2] {
    let mode = layout_id % 8;
    let index = item.source_rank as f64;
    let count_f = count as f64;
    let center = (count_f - 1.0) / 2.0;
    let family_shift = ((layout_id / 64) as f64 * 0.137_503_523_75).fract();
    let shift_fraction = ((((layout_id % 16) + 1) as f64 / 17.0) + family_shift).fract();
    let shift = (count_f * shift_fraction).floor() as i64;
    let stride = if count % 2 == 0 {
        count.saturating_sub(1)
    } else {
        count.saturating_sub(2)
    }
    .max(1) as i64;
    let source_rank = item.source_rank as i64;
    let count_i = count as i64;

    match mode {
        0 => [index, 0.0],
        1 => [count_f - index, 0.0],
        2 => [(index - center).abs(), index],
        3 => [index.max(count_f - index - 1.0), index],
        4 => [positive_modulo(source_rank + shift, count_i) as f64, 0.0],
        5 => [
            positive_modulo((source_rank * stride) + shift, count_i) as f64,
            0.0,
        ],
        6 => [(source_rank % 2) as f64, index],
        _ => [-(source_rank % 2) as f64, count_f - index],
    }
}

fn compare_f64_tuple<const N: usize>(a: &[f64; N], b: &[f64; N]) -> std::cmp::Ordering {
    for index in 0..N {
        let ordering = a[index].total_cmp(&b[index]);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

fn apply_sort_reverse_map_entry(
    patterned: &[u8],
    restored: &mut [u8],
    header: &PatternReverseMapHeader,
    entry: &PatternReverseMapEntry,
    block: &[usize],
) -> Result<()> {
    let items = pattern_items_for_entry(header, entry, block)?;
    let permutation = decode_permutation_factoradic(&entry.encoded, items.len())?;
    let mut by_destination = items.clone();
    by_destination.sort_by(|a, b| {
        compare_f64_tuple(
            &destination_key(a, items.len(), header.layout_id),
            &destination_key(b, items.len(), header.layout_id),
        )
        .then_with(|| a.source_rank.cmp(&b.source_rank))
    });

    for (destination_item, source_rank) in by_destination.iter().zip(permutation.iter().copied()) {
        let Some(source_item) = items.get(source_rank) else {
            bail!("pattern permutation source rank exceeds item count");
        };
        copy_rgb(
            patterned,
            destination_item.pixel_index,
            restored,
            source_item.pixel_index,
        );
    }
    Ok(())
}

fn decode_bool_bits(encoded: &[u8], count: usize) -> Result<Vec<bool>> {
    let expected_len = count.div_ceil(8);
    if encoded.len() != expected_len {
        bail!(
            "pattern bool bit length {} does not match expected {}",
            encoded.len(),
            expected_len
        );
    }
    Ok((0..count)
        .map(|index| encoded[index / 8] & (1 << (7 - (index % 8))) != 0)
        .collect())
}

fn apply_pair_swap_reverse_map_entry(
    patterned: &[u8],
    restored: &mut [u8],
    header: &PatternReverseMapHeader,
    entry: &PatternReverseMapEntry,
    block: &[usize],
) -> Result<()> {
    let items = pattern_items_for_entry(header, entry, block)?;
    let decisions = decode_bool_bits(&entry.encoded, items.len() / 2)?;

    for (pair_index, pair) in items.chunks(2).enumerate() {
        if pair.len() != 2 || !decisions[pair_index] {
            continue;
        }
        copy_rgb(
            patterned,
            pair[1].pixel_index,
            restored,
            pair[0].pixel_index,
        );
        copy_rgb(
            patterned,
            pair[0].pixel_index,
            restored,
            pair[1].pixel_index,
        );
    }
    Ok(())
}

fn restore_patternized_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    descriptor: &RecordDescriptor,
    reverse_map: &PatternReverseMap,
) -> Result<Vec<u8>> {
    if reverse_map.header.mode == PatternReverseMode::DeltaCarrier {
        bail!("delta-carrier pattern maps do not describe a reversible groove sort");
    }
    if reverse_map.header.width != width || reverse_map.header.height != height {
        bail!("pattern reverse map dimensions do not match record PNG");
    }
    if let Some(stream_byte_length) = descriptor.stream_byte_length {
        if reverse_map.header.source_stream_byte_length != stream_byte_length {
            bail!("pattern reverse map stream length does not match record descriptor");
        }
    }

    let selected_indices = pattern_selected_groove_indices(
        width,
        height,
        record_profile,
        descriptor,
        &reverse_map.header,
    )?;
    let mut restored = rgba.to_vec();

    for entry in &reverse_map.entries {
        let block_start = entry
            .block_index
            .checked_mul(reverse_map.header.block_size)
            .context("pattern block start overflow")?;
        if block_start >= selected_indices.len() {
            bail!("pattern reverse map block index exceeds decoded groove blocks");
        }
        let block_end = block_start
            .checked_add(reverse_map.header.block_size)
            .context("pattern block end overflow")?
            .min(selected_indices.len());
        let block = &selected_indices[block_start..block_end];

        match entry.kind {
            PatternReverseEntryKind::Sort => apply_sort_reverse_map_entry(
                rgba,
                &mut restored,
                &reverse_map.header,
                entry,
                block,
            )?,
            PatternReverseEntryKind::PairSwap => apply_pair_swap_reverse_map_entry(
                rgba,
                &mut restored,
                &reverse_map.header,
                entry,
                block,
            )?,
        }
    }

    Ok(restored)
}

fn maybe_restore_patternized_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    record_profile: &str,
    descriptor: &RecordDescriptor,
) -> Result<Option<Vec<u8>>> {
    let Some(reverse_map_bytes) =
        decode_pattern_reverse_map_from_sidecar(rgba, width, height, record_profile, descriptor)?
    else {
        return Ok(None);
    };
    let reverse_map = parse_pattern_reverse_map(&reverse_map_bytes)?;
    restore_patternized_rgba(
        rgba,
        width,
        height,
        record_profile,
        descriptor,
        &reverse_map,
    )
    .map(Some)
}

pub fn infer_record_profile_from_png(png_bytes: &[u8]) -> Result<String> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;

    for &candidate in known_record_profile_names() {
        match decode_record_descriptor_from_rgba(&rgba, width, height, candidate) {
            Ok(descriptor) => {
                if let Some(descriptor_profile) = descriptor.record_profile.as_deref() {
                    let normalized = normalize_record_profile_name(descriptor_profile)?;

                    if normalized == candidate {
                        return Ok(normalized);
                    }
                } else {
                    return Ok(candidate.to_string());
                }
            }
            Err(_) => {}
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

    let descriptor = decode_record_descriptor_from_rgba(&rgba, width, height, &normalized_profile)?;

    if let Some(descriptor_profile) = descriptor.record_profile.as_deref() {
        let normalized_descriptor_profile = normalize_record_profile_name(descriptor_profile)?;

        if normalized_descriptor_profile != normalized_profile {
            bail!(
                "record descriptor profile {} does not match inferred profile {}",
                normalized_descriptor_profile,
                normalized_profile
            );
        }
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
    let descriptor = decode_record_descriptor_from_rgba(&rgba, width, height, &normalized_profile)?;
    let resolved_byte_length = byte_length.or(descriptor.stream_byte_length);
    let dummy_spiral_regions = dummy_spiral_pixel_regions_from_descriptor(&descriptor);
    let restored_rgba =
        maybe_restore_patternized_rgba(&rgba, width, height, &normalized_profile, &descriptor)
            .context("failed to restore patternized record groove")?;
    let groove_rgba = restored_rgba.as_deref().unwrap_or(&rgba);

    let (track_data, pixel_count) = decode_record_groove_to_track_data(
        groove_rgba,
        width,
        height,
        &normalized_profile,
        descriptor.b_value,
        &dummy_spiral_regions,
    )?;

    let bytes = if descriptor.payload_encoding.as_deref() == Some(PAYLOAD_ENCODING_TONED_V1) {
        decode_toned_v1_track(&track_data, resolved_byte_length)?
    } else {
        track_rgba_to_bytes(&track_data, resolved_byte_length)?
    };

    Ok(DecodedChunkStream { bytes, pixel_count })
}

pub const PAYLOAD_ENCODING_TONED_V1: &str = "toned-v1";

/// Decodes a "toned-v1" groove track: a raw-RGB BCS2 prefix (magic + length
/// + metadata JSON) followed by toned spans described by the metadata's
/// first-level `rgbTone` field — an array of
/// `[byteOffset, byteLength, baseRgbHex, lumaTolerance, bitsPerPixel, ordering]`
/// tuples whose byte offsets are relative to the first post-prefix byte.
fn decode_toned_v1_track(track_data: &[u8], byte_length: Option<usize>) -> Result<Vec<u8>> {
    let head_pixels = track_data.len().min(3 * 4);
    let head = track_rgba_to_bytes(&track_data[..head_pixels], None)?;
    if head.len() < 8 || &head[..4] != b"BCS2" {
        bail!("toned-v1 groove does not start with a raw BCS2 prefix");
    }
    let metadata_length = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let raw_length = 8 + metadata_length;
    let raw_pixel_bytes = raw_length.div_ceil(3) * 4;
    if raw_pixel_bytes > track_data.len() {
        bail!("toned-v1 groove is shorter than its BCS2 prefix");
    }

    let mut bytes = track_rgba_to_bytes(&track_data[..raw_pixel_bytes], Some(raw_length))?;
    let metadata: serde_json::Value = serde_json::from_slice(&bytes[8..])
        .context("toned-v1 BCS2 metadata is not valid JSON")?;
    let tuples = metadata
        .get("rgbTone")
        .and_then(|value| value.as_array())
        .context("toned-v1 BCS2 metadata is missing the rgbTone field")?;

    let mut spans = Vec::with_capacity(tuples.len());
    let mut span_byte_offset = 0usize;
    let mut span_pixel_offset = 0usize;
    for tuple in tuples {
        let entry = tuple
            .as_array()
            .filter(|entry| entry.len() >= 6)
            .context("rgbTone span tuple must have at least 6 elements")?;
        let as_usize = |index: usize| -> Result<usize> {
            entry[index]
                .as_u64()
                .map(|value| value as usize)
                .with_context(|| format!("rgbTone tuple element {index} must be an integer"))
        };
        let byte_offset = as_usize(0)?;
        let span_byte_length = as_usize(1)?;
        let hex = entry[2]
            .as_str()
            .context("rgbTone base colour must be a string")?;
        let config = bytes2rgb::TonedConfig {
            base: bytes2rgb::TonedConfig::from_hex(hex, 0, 1).base,
            luma_tolerance: as_usize(3)? as u8,
            bits_per_pixel: as_usize(4)? as u32,
            ordering: if as_usize(5)? == 1 {
                bytes2rgb::ToneOrdering::ChromaProximity
            } else {
                bytes2rgb::ToneOrdering::BaseProximity
            },
        };
        if byte_offset != span_byte_offset {
            bail!("rgbTone spans must be contiguous from byte offset 0");
        }
        let pixel_count = (span_byte_length * 8).div_ceil(config.bits_per_pixel as usize);
        spans.push(bytes2rgb::ToneSpan {
            byte_offset,
            byte_length: span_byte_length,
            pixel_offset: span_pixel_offset,
            pixel_count,
            config,
        });
        span_byte_offset += span_byte_length;
        span_pixel_offset += pixel_count;
    }

    bytes.extend(bytes2rgb::decode_toned_spans(
        &track_data[raw_pixel_bytes..],
        &spans,
    )?);

    if let Some(byte_length) = byte_length {
        if byte_length > bytes.len() {
            bail!("requested byte length exceeds decoded toned payload");
        }
        bytes.truncate(byte_length);
    }
    Ok(bytes)
}

pub fn decode_record_png_to_chunk_stream_for_profile(
    png_bytes: &[u8],
    record_profile: &str,
) -> Result<DecodedChunkStream> {
    decode_record_png_to_chunk_stream_for_profile_with_length(png_bytes, record_profile, None)
}

pub fn decode_record_png_to_chunk_stream_with_length(
    png_bytes: &[u8],
    byte_length: Option<usize>,
) -> Result<(String, DecodedChunkStream)> {
    let (profile, descriptor) = decode_record_descriptor_from_png(png_bytes, None)?;
    let resolved_byte_length = byte_length.or(descriptor.stream_byte_length);
    let decoded = decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        &profile,
        resolved_byte_length,
    )?;

    Ok((profile, decoded))
}

pub fn decode_record_png_to_chunk_stream(png_bytes: &[u8]) -> Result<(String, DecodedChunkStream)> {
    decode_record_png_to_chunk_stream_with_length(png_bytes, None)
}

pub fn decode_record_png(png_bytes: &[u8]) -> Result<DecodedRecord> {
    let (record_profile, descriptor) = decode_record_descriptor_from_png(png_bytes, None)?;
    let chunk_stream = decode_record_png_to_chunk_stream_for_profile_with_length(
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
    use std::path::{Path, PathBuf};

    fn encode_permutation_factoradic_for_test(permutation: &[usize]) -> Vec<u8> {
        let count = permutation.len();
        if count <= 1 {
            return Vec::new();
        }
        let expected_len = permutation_byte_length(count);
        let mut available = (0..count).collect::<Vec<_>>();
        let mut encoded_le = vec![0_u8];

        for &value in permutation {
            let position = available
                .iter()
                .position(|candidate| *candidate == value)
                .expect("test permutation should be valid");
            let base = available.len();
            let mut carry = position;
            for byte in &mut encoded_le {
                let multiplied = (*byte as usize) * base + carry;
                *byte = (multiplied & 0xff) as u8;
                carry = multiplied >> 8;
            }
            while carry > 0 {
                encoded_le.push((carry & 0xff) as u8);
                carry >>= 8;
            }
            available.remove(position);
        }

        encoded_le.resize(expected_len, 0);
        encoded_le.reverse();
        encoded_le
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("record-decode should live one level below the repository root")
            .to_path_buf()
    }

    fn read_fixture(path: impl AsRef<Path>) -> Vec<u8> {
        let path = path.as_ref();

        std::fs::read(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
    }

    fn assert_png_decodes_to_chunk_stream_payload(id: &str, png_name: &str, payload_name: &str) {
        let dir = repo_root().join("goldenfiles").join("records").join(id);
        let png = read_fixture(dir.join(png_name));
        let expected_payload = read_fixture(dir.join(payload_name));

        let (profile, descriptor) = decode_record_descriptor_from_png(&png, None)
            .unwrap_or_else(|error| panic!("{id}/{png_name} descriptor failed: {error:#}"));

        assert_eq!(descriptor.record_profile.as_deref(), Some(profile.as_str()));
        assert!(descriptor.b_value.is_finite());
        assert!(descriptor.b_value > 0.0);
        assert!(descriptor.stream_byte_length.is_some());
        if png_name.contains(".patternized.") {
            assert!(
                descriptor.stego_sidecar_descriptor.is_some(),
                "{id}/{png_name} should carry a pattern reverse-map sidecar pointer"
            );
        }

        let (_profile, decoded) = decode_record_png_to_chunk_stream(&png)
            .unwrap_or_else(|error| panic!("{id}/{png_name} failed to decode: {error:#}"));

        assert_eq!(&decoded.bytes[..4], b"BCS2");

        let stream = record_core::parse_chunk_stream(&decoded.bytes).unwrap_or_else(|error| {
            panic!("{id}/{png_name} decoded invalid chunk stream: {error:#}")
        });
        assert!(
            !stream.chunks.is_empty(),
            "{id}/{png_name} should decode at least one chunk"
        );
        for chunk in &stream.chunks {
            assert_eq!(
                record_core::crc32_ieee(&chunk.payload),
                chunk.crc32,
                "{id}/{png_name} chunk {} CRC32 should validate after pattern reversal",
                chunk.index
            );
        }

        let payload = record_core::chunk_stream_payload_bytes(&stream);

        assert_eq!(
            payload, expected_payload,
            "{id}/{png_name} chunk stream payload should reconstruct {payload_name}"
        );
    }

    fn assert_patternized_groove_restores_canonical(id: &str, patternized_name: &str) {
        let dir = repo_root().join("goldenfiles").join("records").join(id);
        let canonical_png = read_fixture(dir.join(format!("{id}.record.png")));
        let patternized_png = read_fixture(dir.join(patternized_name));

        let (canonical_width, canonical_height, canonical_rgba) =
            load_png_rgba(&canonical_png).unwrap();
        let (pattern_width, pattern_height, pattern_rgba) =
            load_png_rgba(&patternized_png).unwrap();
        assert_eq!(
            (pattern_width, pattern_height),
            (canonical_width, canonical_height)
        );

        let (profile, canonical_descriptor) =
            decode_record_descriptor_from_png(&canonical_png, None).unwrap();
        let pattern_descriptor = decode_record_descriptor_from_rgba(
            &pattern_rgba,
            pattern_width,
            pattern_height,
            &profile,
        )
        .unwrap();
        let restored = maybe_restore_patternized_rgba(
            &pattern_rgba,
            pattern_width,
            pattern_height,
            &profile,
            &pattern_descriptor,
        )
        .unwrap()
        .expect("patternized fixture should restore from sidecar map");

        let canonical_len = canonical_descriptor
            .stream_byte_length
            .expect("canonical fixture should declare stream length");
        let canonical_track = track_rgba_to_bytes(
            &decode_record_groove_to_track_data(
                &canonical_rgba,
                canonical_width,
                canonical_height,
                &profile,
                canonical_descriptor.b_value,
                &[],
            )
            .unwrap()
            .0,
            Some(canonical_len),
        )
        .unwrap();
        let restored_track = track_rgba_to_bytes(
            &decode_record_groove_to_track_data(
                &restored,
                pattern_width,
                pattern_height,
                &profile,
                pattern_descriptor.b_value,
                &[],
            )
            .unwrap()
            .0,
            Some(canonical_len),
        )
        .unwrap();

        if canonical_track != restored_track {
            let first_mismatch = canonical_track
                .iter()
                .zip(restored_track.iter())
                .position(|(a, b)| a != b)
                .unwrap();
            let mask = build_spiral_mask(
                canonical_width,
                canonical_height,
                canonical_descriptor.b_value,
                &profile,
                None,
                None,
                None,
            )
            .unwrap();
            let protected = build_sidecar_protected_metadata_pixels(
                canonical_width,
                canonical_height,
                &profile,
            )
            .unwrap();
            let groove_ordinal = first_mismatch / 3;
            let pixel_index = mask.ordered_pixel_indices[groove_ordinal];
            panic!(
                "{id}/{patternized_name} restored groove differs at byte {first_mismatch} (groove pixel {groove_ordinal}, image pixel {pixel_index}, x={}, y={}, protected={}): canonical={} restored={}",
                pixel_index % canonical_width,
                pixel_index / canonical_width,
                protected[pixel_index],
                canonical_track[first_mismatch],
                restored_track[first_mismatch]
            );
        }
    }

    #[test]
    fn factoradic_pattern_permutations_round_trip() {
        for permutation in [
            vec![0, 1, 2, 3],
            vec![3, 2, 1, 0],
            vec![2, 0, 3, 1],
            (0..128).rev().collect::<Vec<_>>(),
        ] {
            let encoded = encode_permutation_factoradic_for_test(&permutation);
            let decoded = decode_permutation_factoradic(&encoded, permutation.len()).unwrap();
            assert_eq!(decoded, permutation);
        }
    }

    #[test]
    fn checked_in_lp_record_png_decodes_to_chunk_stream_payload() {
        assert_png_decodes_to_chunk_stream_payload(
            "lori-asha-westside-lp-hq",
            "lori-asha-westside-lp-hq.record.png",
            "lori-asha-westside-lp-hq.ecdc",
        );
    }

    #[test]
    fn checked_in_single45_record_png_decodes_to_chunk_stream_payload() {
        assert_png_decodes_to_chunk_stream_payload(
            "lori-asha-westside-single45-hq",
            "lori-asha-westside-single45-hq.record.png",
            "lori-asha-westside-single45-hq.ecdc",
        );
    }

    #[test]
    fn checked_in_lp_patternized_record_png_decodes_to_valid_crc_chunk_stream_payload() {
        assert_patternized_groove_restores_canonical(
            "lori-asha-westside-lp-hq",
            "lori-asha-westside-lp-hq.patternized.record.png",
        );
        assert_png_decodes_to_chunk_stream_payload(
            "lori-asha-westside-lp-hq",
            "lori-asha-westside-lp-hq.patternized.record.png",
            "lori-asha-westside-lp-hq.ecdc",
        );
    }

    #[test]
    fn checked_in_single45_patternized_record_png_decodes_to_valid_crc_chunk_stream_payload() {
        assert_patternized_groove_restores_canonical(
            "lori-asha-westside-single45-hq",
            "lori-asha-westside-single45-hq.patternized.record.png",
        );
        assert_png_decodes_to_chunk_stream_payload(
            "lori-asha-westside-single45-hq",
            "lori-asha-westside-single45-hq.patternized.record.png",
            "lori-asha-westside-single45-hq.ecdc",
        );
    }

    #[test]
    fn checked_in_single45_picture_noise_record_png_decodes_to_chunk_stream_payload() {
        assert_png_decodes_to_chunk_stream_payload(
            "lori-asha-westside-single45-hq",
            "lori-asha-westside-single45-hq.picture-noise.record.png",
            "lori-asha-westside-single45-hq.ecdc",
        );
    }
}
