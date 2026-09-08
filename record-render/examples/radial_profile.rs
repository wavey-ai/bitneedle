//! Ink profile of a groove along a radius.
//!
//!     cargo run --release -p record-render --example radial_profile -- <png> [rays]
//!
//! The example walks out from the centre and records the start radius and the
//! stop radius of each ink run. The requested separation of a cut is centre to
//! centre between turns. The *land* is the gap between two turns after each
//! turn is drawn one pixel wide and antialiased, and the land decides whether
//! the artwork shows. This example prints both figures from measurement,
//! because the land is sub-pixel at these pitches.
use anyhow::Result;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).expect("a png");
    let rays: usize = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(64);
    let img = image::open(&path)?.to_rgba8();
    let (w, h) = (img.width() as f64, img.height() as f64);
    let (cx, cy) = (w / 2.0, h / 2.0);

    let mut on_runs: Vec<f64> = Vec::new();
    let mut off_runs: Vec<f64> = Vec::new();
    let mut first_ink = f64::INFINITY;
    let mut last_ink: f64 = 0.0;
    let step = 0.05;

    for k in 0..rays {
        let a = std::f64::consts::TAU * k as f64 / rays as f64;
        let (dx, dy) = (a.cos(), a.sin());
        let (mut run, mut was) = (0.0, false);
        let mut r = std::env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let stop: f64 = std::env::args().nth(4).and_then(|v| v.parse().ok()).unwrap_or(cx - 1.0);
        while r < stop {
            let (x, y) = ((cx + dx * r) as u32, (cy + dy * r) as u32);
            let ink = img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1)).0[3] > 8;
            if ink {
                first_ink = first_ink.min(r);
                last_ink = last_ink.max(r);
            }
            if ink == was {
                run += step;
            } else {
                if run > 0.0 && r > 4.0 {
                    if was { on_runs.push(run) } else { off_runs.push(run) }
                }
                run = step;
                was = ink;
            }
            r += step;
        }
    }
    let med = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if v.is_empty() { 0.0 } else { v[v.len() / 2] }
    };
    let on_n = on_runs.len();
    let off_n = off_runs.len();
    let (on_med, off_med) = (med(on_runs), med(off_runs));
    println!("{path}");
    println!("  ink from r={first_ink:.0} to r={last_ink:.0}");
    println!("  drawn turn width   median {on_med:.2} px over {on_n} runs");
    println!("  land between turns median {off_med:.2} px over {off_n} runs");
    println!("  so centre to centre is about {:.2} px", on_med + off_med);
    Ok(())
}
