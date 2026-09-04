//! Does the tracer draw the pitch it is given?
//!
//! Builds a mask at a known `b` and measures the drawn separation, with no
//! fit and no payload in the way. If these disagree the fault is in the
//! tracer; if they agree it is in whatever chose `b`.
use anyhow::Result;
use record_core::{build_spiral_mask_with_family, SpiralFamily};

const SIDE: usize = 576;

fn main() -> Result<()> {
    println!("{:>8} {:>10} {:>12} {:>12}", "asked", "b", "drawn c-to-c", "verdict");
    for asked in [1.30_f64, 1.50, 1.75, 2.00, 2.50] {
        let b = asked / std::f64::consts::TAU;
        let mask = build_spiral_mask_with_family(
            SIDE, SIDE, b, &SpiralFamily::Archimedean, "ten", None, None, None,
        )?;
        let mut hit = vec![false; SIDE * SIDE];
        for &i in &mask.ordered_pixel_indices {
            hit[i] = true;
        }
        // Straight up the centre column: a radius, so runs along it are the
        // turn separation with nothing trigonometric in the way.
        let cx = SIDE / 2;
        let (mut runs, mut run, mut was) = (Vec::new(), 0usize, false);
        // radius 145..265 up the centre column: inside the payload band,
        // clear of the label and the rim.
        for y in (23..143).rev() {
            let on = hit[y * SIDE + cx];
            if on == was {
                run += 1;
            } else {
                if run > 0 {
                    runs.push((was, run));
                }
                run = 1;
                was = on;
            }
        }
        let ons: Vec<usize> = runs.iter().filter(|(o, _)| *o).map(|(_, n)| *n).collect();
        let offs: Vec<usize> = runs.iter().filter(|(o, _)| !*o).map(|(_, n)| *n).collect();
        let mean = |v: &Vec<usize>| if v.is_empty() { 0.0 } else { v.iter().sum::<usize>() as f64 / v.len() as f64 };
        let drawn = mean(&ons) + mean(&offs);
        println!(
            "{asked:>8.2} {b:>10.4} {drawn:>12.2} {:>12}",
            if (drawn - asked).abs() < 0.15 { "matches" } else { "DIFFERS" }
        );
    }
    Ok(())
}
