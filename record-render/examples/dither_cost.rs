//! Ink cost of the dither at the 2 px floor.
//!
//! The cut holds a constant pitch. It uses vari-pitch at a depth of 0.001,
//! which is 0.1% of pitch. This depth needs no format change, because an
//! Archimedean cut writes no spiral segment and therefore records no wobble.
//!
//! The example counts the ink off the drawn mask. Phase holds the pitch, so it
//! holds the ink. Amplitude and frequency displace each turn within a land of
//! about one pixel, so they can move the ink.
use anyhow::Result;
use record_core::{build_spiral_mask_with_family, SpiralFamily, VariPitchPlacement, VariPitchTuning};

const SIDE: usize = 576;

fn fam(seed: u64, sheen: f64, grain: f64) -> SpiralFamily {
    SpiralFamily::VariPitch {
        depth: 0.001,
        seed,
        definition: 1.0,
        sheen,
        placement: VariPitchPlacement::Even,
        fire: 0.0,
        tuning: VariPitchTuning {
            wave_one_cycles: 5.4,
            wave_two_cycles: 2.2,
            wave_balance: 0.62,
            dither_frequency: grain,
            aura_width: 0.3,
            fire_cycles: 14.0,
        },
    }
}

fn ink(f: &SpiralFamily, b: f64) -> Result<usize> {
    Ok(build_spiral_mask_with_family(SIDE, SIDE, b, f, "ten", None, None, None)?
        .ordered_pixel_indices
        .len())
}

fn main() -> Result<()> {
    let b = 2.0 / std::f64::consts::TAU;
    let plain = ink(&SpiralFamily::Archimedean, b)?;
    println!("archimedean at 2.0 px: {plain} px of ink\n");

    let report = |label: &str, v: Vec<usize>| {
        let (lo, hi) = (*v.iter().min().unwrap(), *v.iter().max().unwrap());
        println!(
            "{label:<34} {lo} – {hi}   spread {:.3}%   vs plain {:+.2}%",
            (hi - lo) as f64 / plain as f64 * 100.0,
            (hi as f64 / plain as f64 - 1.0) * 100.0
        );
    };

    // Phase only: amplitude and frequency pinned, seed rolling.
    report(
        "phase only (sheen .6, grain 397)",
        (0..24).map(|n| ink(&fam(n * 2_654_435_761 + 7, 0.6, 397.0), b).unwrap()).collect(),
    );
    // Amplitude, at a fixed phase and frequency.
    report(
        "amplitude (sheen 1.0 down to .3)",
        (0..24).map(|n| ink(&fam(7, 1.0 - n as f64 * 0.03, 397.0), b).unwrap()).collect(),
    );
    // Frequency, at a fixed phase and amplitude.
    report(
        "frequency (grain 60 to 1400)",
        (0..24).map(|n| ink(&fam(7, 0.6, 60.0 + n as f64 * 58.0), b).unwrap()).collect(),
    );
    Ok(())
}
