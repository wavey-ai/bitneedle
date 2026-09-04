//! Does a cut below the raster floor actually hold its payload?
//!
//! The fit solves for a pitch that puts N carrier pixels across the band.
//! Below 2 px the tracer rounds turns onto pixels already taken and skips
//! them, so the drawn spiral has fewer addressable pixels than the fit
//! assumed. This asks whether that shortfall is real.
use anyhow::Result;
use record_core::{build_spiral_mask_with_family, SpiralFamily};

const SIDE: usize = 576;

fn main() -> Result<()> {
    println!("{:>10} {:>14} {:>14} {:>10}", "asked px", "addressable", "vs 2.0 px", "per turn");
    for asked in [2.40_f64, 2.20, 2.00, 1.90, 1.81, 1.70, 1.50, 1.30] {
        let b = asked / std::f64::consts::TAU;
        let mask = build_spiral_mask_with_family(
            SIDE, SIDE, b, &SpiralFamily::Archimedean, "ten", None, None, None,
        )?;
        let n = mask.addressable_pixel_count;
        // Turns the pitch implies across the band, against pixels actually
        // laid: if the tracer is losing turns to merging, the pixels per turn
        // will not hold.
        let turns = (279.0 - 138.0) / asked;
        println!("{asked:>10.2} {n:>14} {:>13.1}% {:>10.0}", n as f64 / 92_354.0 * 100.0, n as f64 / turns);
    }
    println!("\nthe toned full side needs 102,025 carrier px");
    Ok(())
}
