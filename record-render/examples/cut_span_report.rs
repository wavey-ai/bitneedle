//! Report how a cut lays out at a range of requested groove spans.
//!
//! `cargo run -p record-render --example cut_span_report -- <profile> <payload.ecdc> <duration>`

use anyhow::Result;
use record_core::describe_record_profile;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let profile = args.get(1).cloned().unwrap_or_else(|| "lp".to_string());
    let payload_path = args.get(2).cloned().unwrap_or_else(|| {
        "goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc".to_string()
    });
    let duration: f64 = args
        .get(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(208.509396);

    let codes = std::fs::read(&payload_path)?;
    let geometry = describe_record_profile(&profile)?;
    let span = (geometry.payload_outer_radius - geometry.payload_inner_radius) as f64;

    println!(
        "profile={profile} payload={} bytes  band {}..{} px (span {span:.0} px)",
        codes.len(),
        geometry.payload_inner_radius,
        geometry.payload_outer_radius,
    );
    println!(
        "{:>10} {:>10} {:>10} {:>12} {:>10} {:>8}",
        "requested", "resolved", "b", "turn gap px", "ends at", "status"
    );

    for requested in [0.25_f64, 0.30, 0.33, 0.40, 0.50, 0.67, 1.0] {
        let options = format!(r#"{{"grooveSpanFraction":{requested}}}"#);
        match record_render::render_payload_codes_to_png(
            &codes,
            "rgb",
            &profile,
            duration,
            Some(&options),
        ) {
            Ok(out) => {
                let p = out.payload;
                println!(
                    "{requested:>10.2} {:>10.3} {:>10.4} {:>12.2} {:>10} {:>8}",
                    p.groove_span_fraction,
                    p.b_value,
                    record_core::turn_separation_px(p.b_value),
                    p.cut_inner_radius,
                    p.status,
                );
            }
            Err(err) => println!("{requested:>10.2} {:>10} {err}", "failed"),
        }
    }

    Ok(())
}
