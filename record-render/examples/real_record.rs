//! Render an actual record — payload, descriptor, lead-out and all — so the
//! lead-out can be looked at on the disc it belongs to rather than on its own.
//!
//! cargo run -p record-render --example real_record -- [profile] [span] [out.png]

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let profile = args.get(1).cloned().unwrap_or_else(|| "lp".to_string());
    let span: f64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.33);
    let out = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| format!("real-{profile}-{span}.png"));

    let payload = args.get(4).cloned().unwrap_or_else(|| {
        let id = if profile.starts_with("single45") {
            "lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.ecdc"
        } else {
            "lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc"
        };
        format!("goldenfiles/records/{id}")
    });
    // The music data can be cut short by truncating the codec bytes: the
    // renderer lays whatever it is given along the spiral, so a fraction of
    // the payload is a shorter programme on the same disc.
    let data_fraction: f64 = args.get(5).and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let full = std::fs::read(&payload)?;
    let keep = ((full.len() as f64) * data_fraction.clamp(0.0, 1.0)).round() as usize;
    let codes = full[..keep.min(full.len())].to_vec();
    let options = format!(r#"{{"grooveSpanFraction":{span},"labelReference":true}}"#);

    let rendered = record_render::render_payload_codes_to_png(
        &codes,
        "rgb",
        &profile,
        208.509396 * data_fraction.clamp(0.0, 1.0),
        Some(&options),
    )?;

    std::fs::write(&out, &rendered.png_bytes)?;

    let geometry = record_core::describe_record_profile(&profile)?;
    let lead_out = record_core::lead_out_geometry_with_extent(
        &profile,
        Some(rendered.payload.cut_inner_radius),
        record_core::LeadOutExtent::Fill,
    )?;
    let scale = record_core::pixels_per_mm(&profile)?;

    println!(
        "profile        {profile}   span {span}   data {:.0}% ({} of {} bytes)",
        data_fraction * 100.0,
        codes.len(),
        full.len()
    );
    println!("cut stops at   {} px", rendered.payload.cut_inner_radius);
    println!(
        "pitch          b={:.5}  ({:.2} px/turn)",
        rendered.payload.b_value,
        2.0 * std::f64::consts::PI * rendered.payload.b_value
    );
    println!("silent_groove        {:.1} turns", rendered.payload.silent_groove_turns);
    println!(
        "lead-out       {:.0} turns, entry {:.1} px, lock {:.1} px  (gaps mm {})",
        lead_out.turns,
        lead_out.entry_radius,
        lead_out.lock_radius,
        lead_out
            .gaps()
            .iter()
            .map(|gap| format!("{:.1}", gap / scale))
            .collect::<Vec<_>>()
            .join(" "),
    );
    println!("label          {} px", geometry.label_radius);
    println!("wrote {out}");

    Ok(())
}
