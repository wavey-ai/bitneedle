use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use std::{env, fs, path::PathBuf};

fn payload_bit(bytes: &[u8], bit_index: usize) -> u8 {
    (bytes[bit_index / 8] >> (7 - bit_index % 8)) & 1
}

fn paint_pair(
    rgba: &mut [u8],
    relief: &[u8],
    first: usize,
    second: usize,
    sign_bit: u8,
    magnitude_bit: u8,
) {
    let first_offset = first * 4;
    let second_offset = second * 4;
    let relief_average = (i16::from(relief[first]) + i16::from(relief[second]) + 1) / 2;
    let common_shift = ((relief_average - 128) as f64 * 0.68).round() as i16;
    let delta = if magnitude_bit == 1 {
        record_sidecar::SIDECAR_PAIR_MAGNITUDE_DELTA
    } else {
        record_sidecar::SIDECAR_PAIR_SIGN_DELTA
    };
    let first_delta = if sign_bit == 1 { delta } else { -delta };
    for channel in 0..3 {
        let local_average = (i16::from(rgba[first_offset + channel])
            + i16::from(rgba[second_offset + channel])
            + 1)
            / 2;
        let embossed_average = (local_average + common_shift).clamp(delta, 255 - delta);
        rgba[first_offset + channel] = (embossed_average + first_delta).clamp(0, 255) as u8;
        rgba[second_offset + channel] = (embossed_average - first_delta).clamp(0, 255) as u8;
    }
}

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let input = args.next().context(
        "usage: pairformed_inscription_spike <record.png> <carrier-mask.png> <relief-mask.png> <image.avif> <encoded.png> <decoded.avif> <report.json>",
    )?;
    let carrier_mask_input = args.next().context("missing inscription carrier mask")?;
    let relief_mask_input = args.next().context("missing inscription relief mask")?;
    let image_input = args.next().context("missing AVIF input")?;
    let output = args.next().context("missing encoded PNG output")?;
    let decoded_output = args.next().context("missing decoded AVIF output")?;
    let report_output = args.next().context("missing report output")?;
    if args.next().is_some() {
        bail!("too many arguments");
    }
    for path in [&output, &decoded_output, &report_output] {
        if path.exists() {
            bail!("refusing to overwrite {}", path.display());
        }
    }

    let source_png = fs::read(&input)?;
    let source_image = fs::read(&image_input)?;
    let mut rgba = image::load_from_memory(&source_png)?.into_rgba8();
    let carrier_mask = image::load_from_memory(&fs::read(&carrier_mask_input)?)?.into_luma8();
    let relief_mask = image::load_from_memory(&fs::read(&relief_mask_input)?)?.into_luma8();
    let (width, height) = rgba.dimensions();
    if carrier_mask.dimensions() != (width, height) || relief_mask.dimensions() != (width, height) {
        bail!("inscription masks must match the record dimensions");
    }

    let seed = 0x46524f4d_u32;
    let options: record_sidecar::SidecarRenderOptions =
        serde_json::from_value(serde_json::json!({
            "scheme": record_sidecar::SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
            "seed": seed,
            "carriers": ["label"],
            "items": [{
                "type": "image",
                "codec": "avif",
                "name": "groove7.avif",
                "mime": "image/avif",
                "dataBase64": URL_SAFE_NO_PAD.encode(&source_image),
            }],
        }))?;
    let sidecar = record_sidecar::prepare_sidecar_render(Some(&options))?
        .context("Sidecar preparation returned no payload")?;

    let mut pairs = Vec::new();
    for y in 0..height {
        let mut x = 0;
        while x + 1 < width {
            if carrier_mask.get_pixel(x, y).0[0] >= 128
                && carrier_mask.get_pixel(x + 1, y).0[0] >= 128
            {
                pairs.push(((y * width + x) as usize, (y * width + x + 1) as usize));
                x += 2;
            } else {
                x += 1;
            }
        }
    }
    record_sidecar::shuffle_pairs_mulberry32(&mut pairs, seed);
    let required_pairs = sidecar.bytes.len().saturating_mul(4);
    if required_pairs > pairs.len() {
        bail!(
            "Sidecar needs {required_pairs} inscription pairs but only {} are available",
            pairs.len()
        );
    }

    let relief = relief_mask.as_raw();
    for (pair_index, &(first, second)) in pairs.iter().enumerate() {
        let first_bit_index = pair_index * 2;
        let sign_bit = if first_bit_index < sidecar.bytes.len() * 8 {
            payload_bit(&sidecar.bytes, first_bit_index)
        } else {
            record_sidecar::metadata_dither(first, pair_index, 41) & 1
        };
        let magnitude_bit = if first_bit_index + 1 < sidecar.bytes.len() * 8 {
            payload_bit(&sidecar.bytes, first_bit_index + 1)
        } else {
            0
        };
        paint_pair(
            rgba.as_mut(),
            relief,
            first,
            second,
            sign_bit,
            magnitude_bit,
        );
    }

    let (decoded_bsc1, decoded_summary) = record_sidecar::decode_sidecar_from_pairs(
        rgba.as_raw(),
        &pairs,
        &sidecar.scheme,
        sidecar.bytes.len(),
    )?;
    if decoded_bsc1 != sidecar.bytes {
        bail!("pair-formed inscription bytes did not round trip");
    }
    let decoded = record_sidecar::decode_sidecar_container_items(&decoded_bsc1)?;
    let item = decoded
        .items
        .iter()
        .find(|item| item.name == "groove7.avif")
        .context("decoded inscription omitted groove7.avif")?;
    let decoded_image =
        record_sidecar::decode_base64_text(&item.data_base64, "decoded groove7.avif")?;
    if decoded_image != source_image {
        bail!("decoded groove7.avif differs from the encoded bytes");
    }

    let mut encoded_png = Vec::new();
    PngEncoder::new(&mut encoded_png).write_image(
        rgba.as_raw(),
        width,
        height,
        ExtendedColorType::Rgba8,
    )?;
    let source_stream = record_decode::decode_record_png_to_chunk_stream(&source_png)?
        .1
        .bytes;
    let encoded_stream = record_decode::decode_record_png_to_chunk_stream(&encoded_png)?
        .1
        .bytes;
    if encoded_stream != source_stream {
        bail!("pair-formed inscription changed the BRS1 payload");
    }

    fs::write(&output, &encoded_png)?;
    fs::write(&decoded_output, &decoded_image)?;
    fs::write(
        &report_output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "prototype": "pair-formed-inscription",
            "stamp": "@fromylvin · 9999",
            "sourceImageBytes": source_image.len(),
            "bsc1Bytes": sidecar.bytes.len(),
            "carrierPairs": pairs.len(),
            "capacityBytes": pairs.len() / 4,
            "usedPairs": required_pairs,
            "visualOperation": "pair common-mode forms relief; pair differential carries sign and magnitude bits",
            "decode": decoded_summary,
            "payloadPreserved": true,
            "imageRoundTrip": true,
            "note": "Experimental carrier ordering requires inscription geometry in a future descriptor version."
        }))?,
    )?;
    println!("pair-formed inscription encoded and verified");
    Ok(())
}
