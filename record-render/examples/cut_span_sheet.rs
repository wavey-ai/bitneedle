//! Render one payload at a range of groove spans, for eyeballing the cut.
//!
//! `cargo run -p record-render --example cut_span_sheet -- <out_dir> <payload> <profile>`

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/tmp/cut-spans".into());
    let payload_path = args.get(2).cloned().expect("payload path required");
    let profile = args.get(3).cloned().unwrap_or_else(|| "lp".into());

    let codes = std::fs::read(&payload_path)?;
    std::fs::create_dir_all(&out_dir)?;

    for requested in [0.25_f64, 0.33, 0.40, 0.50, 0.67, 1.00] {
        let options = format!(r#"{{"grooveSpanFraction":{requested}}}"#);
        let out = record_render::render_payload_codes_to_png(
            &codes,
            "rgb",
            &profile,
            100.0,
            Some(&options),
        )?;
        let name = format!(
            "{out_dir}/span-{:03}.png",
            (requested * 100.0).round() as i32
        );
        std::fs::write(&name, &out.png_bytes)?;
        println!(
            "{requested:.2}\t{:.3}\t{:.4}\t{:.2}\t{}\t{}",
            out.payload.groove_span_fraction,
            out.payload.b_value,
            record_core::turn_separation_px(out.payload.b_value),
            out.payload.cut_inner_radius,
            name,
        );
    }

    Ok(())
}
