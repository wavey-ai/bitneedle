use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let input = args.next().context(
        "usage: inscription_only_spike <stamped-record.png> <mask.png> <image.avif> <encoded.png> <decoded.avif> <report.json>",
    )?;
    let mask_input = args.next().context("missing inscription mask")?;
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
    let mask = image::load_from_memory(&fs::read(&mask_input)?)?.into_luma8();
    let (width, height) = rgba.dimensions();
    if mask.dimensions() != (width, height) {
        bail!("inscription mask dimensions do not match the record");
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
            let first_mask = mask.get_pixel(x, y).0[0];
            let second_mask = mask.get_pixel(x + 1, y).0[0];
            if first_mask >= 128 && second_mask >= 128 {
                pairs.push(((y * width + x) as usize, (y * width + x + 1) as usize));
                x += 2;
            } else {
                x += 1;
            }
        }
    }
    record_sidecar::shuffle_pairs_mulberry32(&mut pairs, seed);

    let summary = record_sidecar::paint_sidecar_bytes_into_pairs(rgba.as_mut(), &pairs, &sidecar)
        .context("inscription pixels could not carry the Sidecar")?;
    let (decoded_bsc1, decoded_summary) = record_sidecar::decode_sidecar_from_pairs(
        rgba.as_raw(),
        &pairs,
        &sidecar.scheme,
        sidecar.bytes.len(),
    )?;
    if decoded_bsc1 != sidecar.bytes {
        bail!("inscription-only pair-sign bytes did not round trip");
    }
    let decoded = record_sidecar::decode_sidecar_container_items(&decoded_bsc1)?;
    let item = decoded
        .items
        .iter()
        .find(|item| item.name == "groove7.avif")
        .context("decoded inscription Sidecar omitted groove7.avif")?;
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
        bail!("inscription-only encoding changed the BRS1 payload");
    }

    fs::write(&output, &encoded_png)?;
    fs::write(&decoded_output, &decoded_image)?;
    fs::write(
        &report_output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "prototype": "inscription-only-pairsign",
            "stamp": "@fromylvin · 9999",
            "mask": mask_input,
            "sourceImageBytes": source_image.len(),
            "sidecar": summary,
            "decode": decoded_summary,
            "payloadPreserved": true,
            "imageRoundTrip": true,
            "note": "Experimental carrier ordering requires inscription geometry in a future descriptor version."
        }))?,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
