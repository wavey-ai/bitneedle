use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let input = args.next().context(
        "usage: inscription_label_spike <stamped-record.png> <image.avif> <encoded.png> <decoded.avif> <report.json>",
    )?;
    let image = args.next().context("missing AVIF input")?;
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

    let source = fs::read(&input).with_context(|| format!("failed to read {}", input.display()))?;
    let source_image =
        fs::read(&image).with_context(|| format!("failed to read {}", image.display()))?;
    let source_stream = record_decode::decode_record_png_to_chunk_stream(&source)
        .context("source record payload did not decode")?
        .1
        .bytes;

    let render_options = serde_json::json!({
        "sidecar": {
            "scheme": record_sidecar::SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
            "seed": 0x46524f4d_u32,
            "carriers": ["label"],
            "labelTuning": { "enabled": false },
            "items": [{
                "type": "image",
                "codec": "avif",
                "name": "groove7.avif",
                "mime": "image/avif",
                "dataBase64": URL_SAFE_NO_PAD.encode(&source_image),
            }],
        },
    });
    let (encoded, summary) =
        record_sidecar::rewrite_record_png(&source, &render_options.to_string(), None)
            .context("label Sidecar rewrite failed")?;

    let encoded_stream = record_decode::decode_record_png_to_chunk_stream(&encoded)
        .context("encoded record payload did not decode")?
        .1
        .bytes;
    if encoded_stream != source_stream {
        bail!("pair-sign label encoding changed the BRS1 payload");
    }

    let decoded_bsc1 = record_sidecar::decode_record_png_sidecar_bytes(&encoded, None)
        .context("encoded label Sidecar did not decode")?;
    let decoded = record_sidecar::decode_sidecar_container_items(&decoded_bsc1)
        .context("decoded Sidecar container was invalid")?;
    let item = decoded
        .items
        .iter()
        .find(|item| item.name == "groove7.avif")
        .context("decoded Sidecar did not contain groove7.avif")?;
    let decoded_image =
        record_sidecar::decode_base64_text(&item.data_base64, "decoded groove7.avif")?;
    if decoded_image != source_image {
        bail!("decoded groove7.avif differs from the encoded bytes");
    }

    fs::write(&output, &encoded)
        .with_context(|| format!("failed to write {}", output.display()))?;
    fs::write(&decoded_output, &decoded_image)
        .with_context(|| format!("failed to write {}", decoded_output.display()))?;
    fs::write(
        &report_output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "stamp": "@fromylvin · 9999",
            "sourceRecordBytes": source.len(),
            "encodedRecordBytes": encoded.len(),
            "sourceImageBytes": source_image.len(),
            "sidecar": summary,
            "payloadPreserved": true,
            "imageRoundTrip": true,
        }))?,
    )
    .with_context(|| format!("failed to write {}", report_output.display()))?;

    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
