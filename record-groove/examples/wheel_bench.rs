//! Times the palettes of a 43-pocket tone wheel, as a wheel press builds them.
use record_groove::{
    adaptive_gap_tone_lightness, lighten_base_oklch, oklch_lightness, TonedConfig, TonedPalette,
};
use std::time::{Duration, Instant};

fn fnv(pal: &TonedPalette, h: &mut u64) {
    for i in 0..pal.len() {
        for c in pal.color(i) {
            *h ^= c as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    }
}

fn main() {
    let count: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(43);
    let bases: Vec<[u8; 3]> = (0..count)
        .map(|i| {
            [
                ((i * 53) % 200 + 40) as u8,
                ((i * 97) % 180 + 30) as u8,
                ((i * 31) % 220 + 20) as u8,
            ]
        })
        .collect();
    let (mut t_bal, mut t_pal, mut t_idx) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut hash = 0xcbf29ce484222325u64;
    let mut tolerances = Vec::new();
    for base in &bases {
        let gap = lighten_base_oklch(
            *base,
            adaptive_gap_tone_lightness(oklch_lightness(*base), 0.35).unwrap(),
        )
        .unwrap();
        for tone in [*base, gap] {
            let t = Instant::now();
            let cfg = TonedConfig::balanced(tone, 1.2).unwrap();
            t_bal += t.elapsed();
            tolerances.push(cfg.luma_tolerance);
            let t = Instant::now();
            let pal = TonedPalette::from_config(cfg).unwrap();
            t_pal += t.elapsed();
            let t = Instant::now();
            assert_eq!(pal.index_of(pal.color(pal.len() - 1)), Some(pal.len() as u32 - 1));
            t_idx += t.elapsed();
            fnv(&pal, &mut hash);
        }
    }
    println!(
        "{} tones: balanced {:.2?} palettes {:.2?} index {:.2?} hash {hash:016x} tolerances {:?}",
        bases.len() * 2,
        t_bal,
        t_pal,
        t_idx,
        tolerances
    );
}
