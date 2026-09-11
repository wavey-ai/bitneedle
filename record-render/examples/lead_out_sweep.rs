//! Synthetic tracks of decreasing length, rendered as whole records, so the
//! lead-out can be watched taking over the room a short track leaves behind.
//!
//! cargo run -p record-render --example lead_out_sweep -- [profile]

use anyhow::Result;
use record_core::LeadOutExtent;

/// Synthetic payload: deterministic bytes, no golden needed.
fn synthetic(byte_count: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    (0..byte_count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 33) as u8
        })
        .collect()
}

/// The extent that a resolved geometry came out as.
fn extent_of(profile: &str, cut: Option<i32>, turns: f64) -> &'static str {
    for (name, extent) in [
        ("Compact", LeadOutExtent::Compact),
        ("Extended", LeadOutExtent::Extended),
        ("Wide", LeadOutExtent::Wide),
        ("ExtraWide", LeadOutExtent::ExtraWide),
        ("Fill", LeadOutExtent::Fill),
    ] {
        if let Ok(geometry) = record_core::lead_out_geometry_with_extent(profile, cut, extent) {
            if (geometry.turns - turns).abs() < 0.01 {
                return name;
            }
        }
    }
    "?"
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let profile = args.get(1).cloned().unwrap_or_else(|| "lp".to_string());
    // A 7"'s recorded band is a third of an album's, so the same payload will
    // not fit at a short span. Size the track to the side it is cut on.
    let bytes: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(120_000);
    let scale = record_core::pixels_per_mm(&profile)?;
    let geometry = record_core::describe_record_profile(&profile)?;

    println!(
        "{profile}: payload band {}..{} px, label {} px\n",
        geometry.payload_inner_radius, geometry.payload_outer_radius, geometry.label_radius
    );
    println!(
        "{:>6} {:>9} {:>9} {:>7} {:>11} {:>9} {:>9}",
        "span", "cut at", "lead-out", "turns", "extent", "entry", "silent_groove"
    );

    for span in [1.0_f64, 0.67, 0.5, 0.33, 0.25, 0.15] {
        let options = format!(r#"{{"grooveSpanFraction":{span}}}"#);
        let rendered = match record_render::render_payload_codes_to_png(
            &synthetic(bytes),
            "rgb",
            &profile,
            208.509396,
            Some(&options),
        ) {
            Ok(rendered) => rendered,
            Err(error) => {
                // A span the payload cannot fit is not a failure of the sweep,
                // it is the answer for that row.
                println!("{span:>6}   {}", error.to_string().lines().next().unwrap_or("failed"));
                continue;
            }
        };

        let cut = match rendered.payload.cut_inner_radius {
            0 => None,
            radius => Some(radius),
        };
        // The band that the record was cut with.
        let lead_out = record_core::lead_out_geometry_with_extent(
            &profile,
            cut,
            LeadOutExtent::Fill,
        )?;
        let name = extent_of(&profile, cut, lead_out.turns);

        let out = format!("sweep-{profile}-{span}.png");
        std::fs::write(&out, &rendered.png_bytes)?;

        println!(
            "{span:>6} {:>9} {:>9.1}mm {:>7.0} {name:>11} {:>8.1}px {:>8.1}t",
            rendered.payload.cut_inner_radius,
            (lead_out.entry_radius - geometry.label_radius as f64) / scale,
            lead_out.turns,
            lead_out.entry_radius,
            rendered.payload.silent_groove_turns,
        );
    }

    Ok(())
}
