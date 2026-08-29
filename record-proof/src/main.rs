//! `record-proof <record.png> [out.png] [--json]`
//!
//! Post-processes a record PNG into a print proof: same disc, transparent
//! background, colour calibration targets in the four corners.

use anyhow::{bail, Context, Result};
use record_proof::add_print_calibration;
use std::path::{Path, PathBuf};

fn default_output(input: &Path) -> PathBuf {
    let name = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("record.png");
    let stem = name
        .strip_suffix(".record.png")
        .or_else(|| name.strip_suffix(".png"))
        .unwrap_or(name);
    input.with_file_name(format!("{stem}.proof.png"))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut json = false;
    for arg in args.by_ref() {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("usage: record-proof <record.png> [out.png] [--json]");
                return Ok(());
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ if output.is_none() => output = Some(PathBuf::from(arg)),
            _ => bail!("unexpected argument {arg}"),
        }
    }
    let input = input.context("usage: record-proof <record.png> [out.png] [--json]")?;
    let output = output.unwrap_or_else(|| default_output(&input));

    let png = std::fs::read(&input).with_context(|| format!("reading {}", input.display()))?;
    let proof = add_print_calibration(&png)?;
    std::fs::write(&output, &proof.png).with_context(|| format!("writing {}", output.display()))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "output": output,
                "config": proof.config,
                "stats": proof.stats,
            }))?
        );
    } else {
        println!(
            "{} -> {} ({} px painted, {} swatch cells/corner, {}px ring width, {} tone span palette(s), {}-byte QR config)",
            input.display(),
            output.display(),
            proof.stats.painted_pixels,
            proof.stats.swatch_cells,
            proof.config.ring_width,
            proof.config.tone_spans.len(),
            proof.stats.qr_bytes,
        );
    }
    Ok(())
}
