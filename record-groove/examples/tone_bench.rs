use record_groove::{TonedConfig, TonedPalette};
use std::time::Instant;

fn fnv(pal: &TonedPalette) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for i in 0..pal.len() {
        for c in pal.color(i) {
            h ^= c as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

fn main() {
    let bases: [[u8; 3]; 5] = [[0xF2, 0xEE, 0xE5], [0xFF, 0xC0, 0xCB], [0x10, 0x18, 0x40], [0x80, 0x80, 0x80], [0x1E, 0x0B, 0x2E]];
    let factors: Vec<f64> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let factors = if factors.is_empty() { vec![1.2] } else { factors };
    let payload = vec![0xA5u8; 140_000];
    for &factor in &factors {
        for base in bases {
            let t = Instant::now();
            let cfg = TonedConfig::balanced(base, factor).unwrap();
            let t_bal = t.elapsed();
            let t = Instant::now();
            let pal = TonedPalette::from_config(cfg).unwrap();
            let t_pal = t.elapsed();
            let t = Instant::now();
            let rgba = pal.bytes_to_rgba(&payload);
            let t_enc = t.elapsed();
            let t = Instant::now();
            let back = pal.rgba_to_bytes(&rgba, Some(payload.len())).unwrap();
            let t_dec = t.elapsed();
            assert_eq!(back, payload);
            let mean = pal.mean_color();
            println!(
                "x{factor} {:02X}{:02X}{:02X}: balanced {:>7.1?} (tol ±{:<3} bpp {}) palette {:>6.1?} enc {:>7.1?} dec {:>6.1?} px {} mean {:02X}{:02X}{:02X} drift {:.0} hash {:016x}",
                base[0], base[1], base[2], t_bal, cfg.luma_tolerance, cfg.bits_per_pixel, t_pal, t_enc, t_dec, rgba.len() / 4,
                mean[0], mean[1], mean[2], pal.max_drift(), fnv(&pal)
            );
        }
    }
}
