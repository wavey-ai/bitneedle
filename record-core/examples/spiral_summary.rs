use anyhow::{anyhow, Result};
use record_core::{build_spiral_mask, describe_record_profile};
use serde_json::json;
use sha2::{Digest, Sha256};

fn parse_arg<T>(args: &[String], index: usize, fallback: T, label: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match args.get(index) {
        Some(value) => value
            .parse()
            .map_err(|error| anyhow!("failed to parse {label}: {value}: {error}")),
        None => Ok(fallback),
    }
}

fn sha256_indices(indices: &[usize]) -> String {
    let mut hasher = Sha256::new();
    for &index in indices {
        hasher.update((index as u32).to_be_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let width = parse_arg(&args, 1, 576usize, "width")?;
    let height = parse_arg(&args, 2, 576usize, "height")?;
    let b_value = parse_arg(&args, 3, 0.25f64, "b_value")?;
    let record_profile = args.get(4).map(String::as_str).unwrap_or("single45");

    let geometry = describe_record_profile(record_profile)?;
    let mask = build_spiral_mask(width, height, b_value, record_profile, None, None, None)?;
    let first = mask
        .ordered_pixel_indices
        .iter()
        .copied()
        .take(32)
        .collect::<Vec<_>>();
    let mut last = mask
        .ordered_pixel_indices
        .iter()
        .rev()
        .copied()
        .take(32)
        .collect::<Vec<_>>();
    last.reverse();

    println!(
        "{}",
        serde_json::to_string(&json!({
            "width": width,
            "height": height,
            "bValue": b_value,
            "recordProfile": record_profile,
            "geometry": geometry,
            "addressablePixelCount": mask.addressable_pixel_count,
            "indexSha256": sha256_indices(&mask.ordered_pixel_indices),
            "first": first,
            "last": last,
        }))?
    );

    Ok(())
}
