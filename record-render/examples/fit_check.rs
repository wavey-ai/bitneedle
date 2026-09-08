//! Payload capacity of a cut below the raster floor.
//!
//! The fit solves for a pitch that puts N carrier pixels across the band.
//! Below 2 px the tracer rounds turns onto taken pixels and skips them, so the
//! drawn spiral holds fewer addressable pixels than the fit assumed. This
//! example measures that shortfall.
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
        // The turns that the pitch implies across the band, against the laid
        // pixels. A tracer that loses turns to merging changes the pixels per
        // turn.
        let turns = (279.0 - 138.0) / asked;
        println!("{asked:>10.2} {n:>14} {:>13.1}% {:>10.0}", n as f64 / 92_354.0 * 100.0, n as f64 / turns);
    }
    println!("\nthe toned full side needs 102,025 carrier px");
    Ok(())
}
