//! Report the sidecar carrier plan of a record, tier by tier.
//!
//! cargo run -p record-sidecar --example carrier_plan -- <record.png>

use anyhow::Result;
use record_sidecar::{
    build_sidecar_plan, sidecar_bit_capacity_for_pairs, GrooveEncoding, SidecarRun,
    SIDECAR_DEFAULT_SEED, SIDECAR_FILL_ORDER,
};

fn main() -> Result<()> {
    let scheme = "pairsign-safe-luma-v2";
    for path in std::env::args().skip(1) {
        let png = std::fs::read(&path)?;
        let (profile, bytes) = record_decode::decode_record_descriptor_bytes_from_png(&png, None)?;
        let descriptor = record_descriptor::decode_record_descriptor_bytes(&bytes)?;
        let (width, height, rgba) = record_decode::load_record_rgba(&png)?;

        println!("\n=== {path}");
        println!("profile {profile}   descriptor {} bytes   cut_inner_radius {}",
            bytes.len(), descriptor.cut_inner_radius);

        let plan = build_sidecar_plan(
            width,
            height,
            &descriptor,
            bytes.len(),
            &SIDECAR_FILL_ORDER,
            SIDECAR_DEFAULT_SEED,
            None,
        )?;

        println!("{:>12} {:>8} {:>10} {:>12} {:>10}",
            "carrier", "kind", "units", "bits/unit", "bytes");
        let mut total = 0usize;
        for run in &plan {
            let (kind, units, per, capacity) = match run {
                SidecarRun::Groove { indices, encoding, .. } => {
                    let bits = match encoding {
                        GrooveEncoding::Grey => 6,
                        GrooveEncoding::Toned(clock) => clock.bits_per_pixel as usize,
                    };
                    ("groove", indices.len(), bits, encoding.byte_capacity(indices.len()))
                }
                SidecarRun::Steg { pairs, .. } => {
                    let bits = sidecar_bit_capacity_for_pairs(scheme, pairs, &rgba)?;
                    ("steg", pairs.len(), 0, bits / 8)
                }
            };
            total += capacity;
            println!("{:>12} {kind:>8} {units:>10} {per:>12} {capacity:>10}",
                run.carrier().name(), );
        }
        println!("{:>12} {:>8} {:>10} {:>12} {total:>10}", "TOTAL", "", "", "");
    }
    Ok(())
}
