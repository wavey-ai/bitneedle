//! Reversible, seeded colour-streak permutation over caller-selected pixels.
//!
//! The caller owns image geometry and selects the ordered pixel indices. The
//! returned BNPM blob contains everything required to restore those pixels
//! exactly, but contains no image- or record-specific state.

use anyhow::{bail, Context, Result};
use serde::Serialize;

const MAGIC: &[u8; 4] = b"BNPM";
const VERSION: u8 = 3;
const HEADER_LEN: usize = 28;
const ENTRY_HEADER_LEN: usize = 6;

#[derive(Debug, Clone, Copy)]
pub struct PatternizeOptions {
    pub seed: u32,
    pub amount: f64,
    pub block_size: usize,
    pub channels: usize,
}

impl Default for PatternizeOptions {
    fn default() -> Self {
        Self {
            seed: 0,
            amount: 0.5,
            block_size: 128,
            channels: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualScore {
    pub score: f64,
    pub base_score: f64,
    pub similar_edges: usize,
    pub bright_similar_edges: usize,
    pub edge_count: usize,
    pub similar_percent: f64,
    pub bright_similar_percent: f64,
    pub longest_run: usize,
    pub bright_longest_run: usize,
    pub bright_dominance: f64,
    pub mean_run: f64,
    pub run_count: usize,
    pub mean_similarity: f64,
    pub mean_bright_similarity: f64,
    pub mean_brightness: f64,
    pub threshold: f64,
    pub bright_threshold: f64,
}

fn normalize_amount(amount: f64) -> f64 {
    let fraction = if amount > 1.0 { amount / 100.0 } else { amount };
    fraction.clamp(0.01, 1.0)
}

fn block_hash_unit(block_index: u64, seed: u64) -> f64 {
    let mut value = block_index
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(seed.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(0x94d0_49bb_1331_11eb);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value as f64) / (u64::MAX as f64)
}

fn luma(red: u8, green: u8, blue: u8) -> f64 {
    red as f64 * 0.2126 + green as f64 * 0.7152 + blue as f64 * 0.0722
}

fn hue(red: u8, green: u8, blue: u8) -> f64 {
    let r = red as f64 / 255.0;
    let g = green as f64 / 255.0;
    let b = blue as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta == 0.0 {
        return 0.0;
    }
    let value = if max == r {
        ((g - b) / delta) % 6.0
    } else if max == g {
        ((b - r) / delta) + 2.0
    } else {
        ((r - g) / delta) + 4.0
    };
    if value < 0.0 {
        value + 6.0
    } else {
        value
    }
}

fn saturation(red: u8, green: u8, blue: u8) -> f64 {
    let max = red.max(green).max(blue) as f64;
    let min = red.min(green).min(blue) as f64;
    if max == 0.0 {
        0.0
    } else {
        (max - min) / max
    }
}

fn color_sort_key(red: u8, green: u8, blue: u8, seed: u32) -> [f64; 6] {
    let luma = luma(red, green, blue);
    let hue = hue(red, green, blue);
    let saturation = saturation(red, green, blue);
    let (red, green, blue) = (red as f64, green as f64, blue as f64);
    let family = seed / 64;
    let shifted_hue = (hue + (family as f64 * 0.381_966_011_25) % 6.0) % 6.0;
    let luma_variant = if family.is_multiple_of(2) {
        luma
    } else {
        255.0 - luma
    };
    let saturation_variant = if family.is_multiple_of(3) {
        saturation
    } else {
        1.0 - saturation
    };
    let channel_mix = match family % 4 {
        1 => red * 0.55 + green * 0.30 + blue * 0.15,
        2 => green * 0.55 + blue * 0.30 + red * 0.15,
        3 => blue * 0.55 + red * 0.30 + green * 0.15,
        _ => red,
    };
    match (seed / 8) % 8 {
        0 => [
            luma_variant,
            shifted_hue,
            saturation_variant,
            red,
            green,
            blue,
        ],
        1 => [
            shifted_hue,
            saturation_variant,
            luma_variant,
            red,
            green,
            blue,
        ],
        2 => [
            saturation_variant,
            shifted_hue,
            luma_variant,
            red,
            green,
            blue,
        ],
        3 => [
            red,
            green,
            blue,
            luma_variant,
            shifted_hue,
            saturation_variant,
        ],
        4 => [
            green,
            blue,
            red,
            luma_variant,
            shifted_hue,
            saturation_variant,
        ],
        5 => [
            blue,
            red,
            green,
            luma_variant,
            shifted_hue,
            saturation_variant,
        ],
        6 => [
            red - blue,
            luma_variant,
            shifted_hue,
            saturation_variant,
            green,
            red,
        ],
        _ => [
            channel_mix,
            shifted_hue,
            luma_variant,
            saturation_variant,
            red,
            green,
        ],
    }
}

fn destination_key(source_rank: usize, count: usize, seed: u32) -> [f64; 2] {
    let mode = seed % 8;
    let index = source_rank as f64;
    let count_f = count as f64;
    let center = (count_f - 1.0) / 2.0;
    let family_shift = ((seed / 64) as f64 * 0.137_503_523_75).fract();
    let shift_fraction = ((((seed % 16) + 1) as f64 / 17.0) + family_shift).fract();
    let shift = (count_f * shift_fraction).floor() as i64;
    let stride = if count.is_multiple_of(2) {
        count.saturating_sub(1)
    } else {
        count.saturating_sub(2)
    }
    .max(1) as i64;
    let rank = source_rank as i64;
    let count_i = count as i64;
    match mode {
        0 => [index, 0.0],
        1 => [count_f - index, 0.0],
        2 => [(index - center).abs(), index],
        3 => [index.max(count_f - index - 1.0), index],
        4 => [(rank + shift).rem_euclid(count_i) as f64, 0.0],
        5 => [((rank * stride) + shift).rem_euclid(count_i) as f64, 0.0],
        6 => [(rank % 2) as f64, index],
        _ => [-(rank % 2) as f64, count_f - index],
    }
}

fn compare_tuple<const N: usize>(a: &[f64; N], b: &[f64; N]) -> std::cmp::Ordering {
    for index in 0..N {
        let ordering = a[index].total_cmp(&b[index]);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

fn permutation_byte_length(count: usize) -> usize {
    let bits = (2..=count).fold(0.0, |total, value| total + (value as f64).log2());
    (bits / 8.0).ceil() as usize
}

fn encode_permutation(permutation: &[usize]) -> Result<Vec<u8>> {
    if permutation.len() <= 1 {
        return Ok(Vec::new());
    }
    let expected_len = permutation_byte_length(permutation.len());
    let mut available = (0..permutation.len()).collect::<Vec<_>>();
    let mut encoded_le = vec![0_u8];
    for &value in permutation {
        let position = available
            .iter()
            .position(|candidate| *candidate == value)
            .context("patternize permutation is invalid")?;
        let base = available.len();
        let mut carry = position;
        for byte in &mut encoded_le {
            let value = (*byte as usize)
                .checked_mul(base)
                .and_then(|value| value.checked_add(carry))
                .context("patternize permutation encoding overflow")?;
            *byte = (value & 0xff) as u8;
            carry = value >> 8;
        }
        while carry > 0 {
            encoded_le.push((carry & 0xff) as u8);
            carry >>= 8;
        }
        available.remove(position);
    }
    if encoded_le.len() > expected_len {
        bail!("patternize permutation exceeds its encoded length");
    }
    encoded_le.resize(expected_len, 0);
    encoded_le.reverse();
    Ok(encoded_le)
}

fn divide_big_endian(number: &mut [u8], divisor: usize) -> Result<usize> {
    if divisor == 0 {
        bail!("cannot divide patternize permutation by zero");
    }
    let mut carry = 0usize;
    for byte in number {
        let value = carry
            .checked_mul(256)
            .and_then(|value| value.checked_add(*byte as usize))
            .context("patternize permutation division overflow")?;
        *byte = (value / divisor) as u8;
        carry = value % divisor;
    }
    Ok(carry)
}

fn decode_permutation(encoded: &[u8], count: usize) -> Result<Vec<usize>> {
    if count <= 1 {
        return if count == 0 {
            Ok(Vec::new())
        } else if encoded.is_empty() {
            Ok(vec![0])
        } else {
            bail!("single-item permutation should be empty")
        };
    }
    if encoded.len() != permutation_byte_length(count) {
        bail!("patternize permutation length mismatch");
    }
    let mut number = encoded.to_vec();
    let mut positions = Vec::with_capacity(count);
    for base in 1..=count {
        positions.push(divide_big_endian(&mut number, base)?);
    }
    if number.iter().any(|byte| *byte != 0) {
        bail!("patternize permutation is outside the expected range");
    }
    positions.reverse();
    let mut available = (0..count).collect::<Vec<_>>();
    let mut permutation = Vec::with_capacity(count);
    for position in positions {
        if position >= available.len() {
            bail!("patternize permutation position is invalid");
        }
        permutation.push(available.remove(position));
    }
    Ok(permutation)
}

fn copy_pixel(
    source: &[u8],
    source_index: usize,
    target: &mut [u8],
    target_index: usize,
    channels: usize,
) {
    let source_offset = source_index * channels;
    let target_offset = target_index * channels;
    target[target_offset..target_offset + channels]
        .copy_from_slice(&source[source_offset..source_offset + channels]);
}

fn validate_indices(pixels: &[u8], indices: &[usize], channels: usize) -> Result<()> {
    if channels < 3 {
        bail!("patternize requires at least three channels");
    }
    for &index in indices {
        if (index + 1).saturating_mul(channels) > pixels.len() {
            bail!("patternize pixel index {index} is out of bounds");
        }
    }
    Ok(())
}

pub fn patternize(
    pixels: &mut [u8],
    indices: &[usize],
    options: &PatternizeOptions,
) -> Result<Vec<u8>> {
    validate_indices(pixels, indices, options.channels)?;
    let amount = normalize_amount(options.amount);
    let block_size = options.block_size.max(2);
    let source = pixels.to_vec();
    let mut output = source.clone();
    let mut entries = Vec::new();
    for (block_index, block) in indices.chunks(block_size).enumerate() {
        if block.len() <= 1 || block_hash_unit(block_index as u64, options.seed as u64) >= amount {
            continue;
        }
        let mut by_color = (0..block.len()).collect::<Vec<_>>();
        by_color.sort_by(|&a, &b| {
            let offset_a = block[a] * options.channels;
            let offset_b = block[b] * options.channels;
            compare_tuple(
                &color_sort_key(
                    source[offset_a],
                    source[offset_a + 1],
                    source[offset_a + 2],
                    options.seed,
                ),
                &color_sort_key(
                    source[offset_b],
                    source[offset_b + 1],
                    source[offset_b + 2],
                    options.seed,
                ),
            )
            .then_with(|| a.cmp(&b))
        });
        let mut by_destination = (0..block.len()).collect::<Vec<_>>();
        by_destination.sort_by(|&a, &b| {
            compare_tuple(
                &destination_key(a, block.len(), options.seed),
                &destination_key(b, block.len(), options.seed),
            )
            .then_with(|| a.cmp(&b))
        });
        for (&source_rank, &destination_rank) in by_color.iter().zip(&by_destination) {
            copy_pixel(
                &source,
                block[source_rank],
                &mut output,
                block[destination_rank],
                options.channels,
            );
        }
        entries.push((block_index, block.len(), encode_permutation(&by_color)?));
    }
    pixels.copy_from_slice(&output);
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut map = vec![0_u8; HEADER_LEN];
    map[..4].copy_from_slice(MAGIC);
    map[4] = VERSION;
    map[5] = u8::try_from(options.channels).context("patternize channels exceed u8 range")?;
    map[8..12].copy_from_slice(&options.seed.to_be_bytes());
    map[12..16].copy_from_slice(&((amount * 1_000_000.0).round() as u32).to_be_bytes());
    map[16..20].copy_from_slice(&(u32::try_from(block_size)?).to_be_bytes());
    map[20..24].copy_from_slice(&(u32::try_from(indices.len())?).to_be_bytes());
    map[24..28].copy_from_slice(&(u32::try_from(entries.len())?).to_be_bytes());
    for (block_index, block_len, permutation) in entries {
        map.extend_from_slice(&u32::try_from(block_index)?.to_be_bytes());
        map.extend_from_slice(&u16::try_from(block_len)?.to_be_bytes());
        map.extend_from_slice(&permutation);
    }
    Ok(map)
}

fn read_u32(bytes: &[u8], offset: usize, label: &str) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .with_context(|| format!("{label} is truncated"))?
            .try_into()
            .unwrap(),
    ))
}

pub fn reverse_map_amount(reverse_map: &[u8]) -> Result<f64> {
    if reverse_map.len() < HEADER_LEN || &reverse_map[..4] != MAGIC || reverse_map[4] != VERSION {
        bail!("invalid or unsupported patternize reverse map");
    }
    Ok(read_u32(reverse_map, 12, "patternize amount")? as f64 / 1_000_000.0)
}

pub fn restore(pixels: &mut [u8], indices: &[usize], reverse_map: &[u8]) -> Result<()> {
    if reverse_map.is_empty() {
        return Ok(());
    }
    if reverse_map.len() < HEADER_LEN || &reverse_map[..4] != MAGIC || reverse_map[4] != VERSION {
        bail!("invalid or unsupported patternize reverse map");
    }
    let channels = reverse_map[5] as usize;
    validate_indices(pixels, indices, channels)?;
    let seed = read_u32(reverse_map, 8, "patternize seed")?;
    let block_size = read_u32(reverse_map, 16, "patternize block size")? as usize;
    let pixel_count = read_u32(reverse_map, 20, "patternize pixel count")? as usize;
    let entry_count = read_u32(reverse_map, 24, "patternize entry count")? as usize;
    if pixel_count != indices.len() || block_size < 2 {
        bail!("patternize reverse map does not match selected pixels");
    }
    let patterned = pixels.to_vec();
    let mut restored = patterned.clone();
    let mut offset = HEADER_LEN;
    for _ in 0..entry_count {
        if offset + ENTRY_HEADER_LEN > reverse_map.len() {
            bail!("patternize entry header is truncated");
        }
        let block_index = read_u32(reverse_map, offset, "patternize block index")? as usize;
        let block_len =
            u16::from_be_bytes(reverse_map[offset + 4..offset + 6].try_into().unwrap()) as usize;
        offset += ENTRY_HEADER_LEN;
        let body_len = permutation_byte_length(block_len);
        if offset + body_len > reverse_map.len() {
            bail!("patternize entry is truncated");
        }
        let permutation = decode_permutation(&reverse_map[offset..offset + body_len], block_len)?;
        offset += body_len;
        let block_start = block_index
            .checked_mul(block_size)
            .context("patternize block offset overflow")?;
        let block_end = block_start.saturating_add(block_size).min(indices.len());
        let block = indices
            .get(block_start..block_end)
            .context("patternize block index exceeds selected pixels")?;
        if block.len() != block_len {
            bail!("patternize block length does not match selected pixels");
        }
        let mut by_destination = (0..block.len()).collect::<Vec<_>>();
        by_destination.sort_by(|&a, &b| {
            compare_tuple(
                &destination_key(a, block.len(), seed),
                &destination_key(b, block.len(), seed),
            )
            .then_with(|| a.cmp(&b))
        });
        for (&destination_rank, source_rank) in by_destination.iter().zip(permutation) {
            copy_pixel(
                &patterned,
                block[destination_rank],
                &mut restored,
                block[source_rank],
                channels,
            );
        }
    }
    if offset != reverse_map.len() {
        bail!("patternize reverse map has trailing bytes");
    }
    pixels.copy_from_slice(&restored);
    Ok(())
}

fn pixel_luma(pixels: &[u8], index: usize, channels: usize) -> f64 {
    let offset = index * channels;
    luma(pixels[offset], pixels[offset + 1], pixels[offset + 2])
}

fn rgb_distance(pixels: &[u8], first: usize, second: usize, channels: usize) -> f64 {
    let first = first * channels;
    let second = second * channels;
    let dr = pixels[first] as f64 - pixels[second] as f64;
    let dg = pixels[first + 1] as f64 - pixels[second + 1] as f64;
    let db = pixels[first + 2] as f64 - pixels[second + 2] as f64;
    (dr * dr * 0.2126 + dg * dg * 0.7152 + db * db * 0.0722).sqrt()
}

pub fn visual_score(pixels: &[u8], indices: &[usize], channels: usize) -> VisualScore {
    let threshold = 34.0;
    let bright_threshold = 132.0;
    if indices.len() < 2 || validate_indices(pixels, indices, channels).is_err() {
        return VisualScore {
            score: 0.0,
            base_score: 0.0,
            similar_edges: 0,
            bright_similar_edges: 0,
            edge_count: 0,
            similar_percent: 0.0,
            bright_similar_percent: 0.0,
            longest_run: indices.len(),
            bright_longest_run: 0,
            bright_dominance: 0.0,
            mean_run: indices.len() as f64,
            run_count: indices.len(),
            mean_similarity: 0.0,
            mean_bright_similarity: 0.0,
            mean_brightness: 0.0,
            threshold,
            bright_threshold,
        };
    }
    let mut similar_edges = 0usize;
    let mut bright_similar_edges = 0usize;
    let mut current_run = 1usize;
    let mut current_luma = pixel_luma(pixels, indices[0], channels);
    let mut longest_run = 1usize;
    let mut bright_longest_run = 0usize;
    let mut bright_run_total = 0usize;
    let mut run_count = 0usize;
    let mut run_total = 0usize;
    let mut weighted_runs = 0.0;
    let mut bright_weighted_runs = 0.0;
    let mut similarity_total = 0.0;
    let mut bright_similarity_total = 0.0;
    let mut luma_total = current_luma;
    for pair in indices.windows(2) {
        let first_luma = pixel_luma(pixels, pair[0], channels);
        let second_luma = pixel_luma(pixels, pair[1], channels);
        let edge_luma = (first_luma + second_luma) * 0.5;
        let brightness = (edge_luma / 255.0).clamp(0.0, 1.0).powf(1.7);
        let distance = rgb_distance(pixels, pair[0], pair[1], channels);
        let similarity = (1.0 - distance / 96.0).clamp(0.0, 1.0);
        similarity_total += similarity;
        bright_similarity_total += similarity * brightness;
        luma_total += second_luma;
        if distance <= threshold {
            similar_edges += 1;
            if edge_luma >= bright_threshold {
                bright_similar_edges += 1;
            }
            current_run += 1;
            current_luma += second_luma;
        } else {
            longest_run = longest_run.max(current_run);
            run_total += current_run;
            run_count += 1;
            weighted_runs += (current_run as f64).powf(1.45);
            let mean = current_luma / current_run as f64;
            bright_weighted_runs +=
                (current_run as f64).powf(1.55) * (mean / 255.0).clamp(0.0, 1.0).powf(1.8);
            if mean >= bright_threshold {
                bright_run_total += current_run;
                bright_longest_run = bright_longest_run.max(current_run);
            }
            current_run = 1;
            current_luma = second_luma;
        }
    }
    longest_run = longest_run.max(current_run);
    run_total += current_run;
    run_count += 1;
    weighted_runs += (current_run as f64).powf(1.45);
    let final_mean = current_luma / current_run as f64;
    bright_weighted_runs +=
        (current_run as f64).powf(1.55) * (final_mean / 255.0).clamp(0.0, 1.0).powf(1.8);
    if final_mean >= bright_threshold {
        bright_run_total += current_run;
        bright_longest_run = bright_longest_run.max(current_run);
    }
    let edges = indices.len() - 1;
    let similar = similar_edges as f64 / edges as f64;
    let bright_similar = bright_similar_edges as f64 / edges as f64;
    let bright_dominance = bright_run_total as f64 / indices.len() as f64;
    let mean_similarity = similarity_total / edges as f64;
    let mean_bright_similarity = bright_similarity_total / edges as f64;
    let normalized_runs = weighted_runs / indices.len() as f64;
    let normalized_bright_runs = bright_weighted_runs / indices.len() as f64;
    let base_score = normalized_runs * 240.0 + similar * 320.0 + mean_similarity * 180.0;
    let score = normalized_bright_runs * 410.0
        + bright_dominance * 360.0
        + bright_similar * 220.0
        + mean_bright_similarity * 180.0
        + mean_similarity * 90.0
        + similar * 100.0
        + normalized_runs * 80.0;
    VisualScore {
        score: (score * 100.0).round() / 100.0,
        base_score: (base_score * 100.0).round() / 100.0,
        similar_edges,
        bright_similar_edges,
        edge_count: edges,
        similar_percent: (similar * 1000.0).round() / 10.0,
        bright_similar_percent: (bright_similar * 1000.0).round() / 10.0,
        longest_run,
        bright_longest_run,
        bright_dominance: (bright_dominance * 1000.0).round() / 10.0,
        mean_run: ((run_total as f64 / run_count as f64) * 10.0).round() / 10.0,
        run_count,
        mean_similarity: (mean_similarity * 1000.0).round() / 1000.0,
        mean_bright_similarity: (mean_bright_similarity * 1000.0).round() / 1000.0,
        mean_brightness: ((luma_total / indices.len() as f64) * 10.0).round() / 10.0,
        threshold,
        bright_threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, Rng, SeedableRng};

    #[test]
    fn rgba_round_trip_is_exact() {
        let mut rng = StdRng::seed_from_u64(42);
        let original = (0..800).map(|_| rng.gen()).collect::<Vec<u8>>();
        let mut working = original.clone();
        let indices = (0..200).collect::<Vec<_>>();
        let map = patternize(
            &mut working,
            &indices,
            &PatternizeOptions {
                seed: 17,
                amount: 0.7,
                block_size: 32,
                channels: 4,
            },
        )
        .unwrap();
        assert_ne!(working, original);
        restore(&mut working, &indices, &map).unwrap();
        assert_eq!(working, original);
    }
}
