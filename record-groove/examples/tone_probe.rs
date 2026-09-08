use record_groove::{TonedConfig, TonedPalette};

/// sRGB -> OKLab, so a difference can be stated in a perceptually even space.
fn oklab(c: [u8; 3]) -> (f64, f64, f64) {
    let f = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (f(c[0]), f(c[1]), f(c[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    (
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )
}

fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (l1, a1, b1) = oklab(a);
    let (l2, a2, b2) = oklab(b);
    (((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()) * 100.0
}

fn main() {
    const FACTOR: f64 = 1.2;
    println!("dE in OKLab x100 (JND ~ 2.0 on this scale)\n");
    for base in [[150u8, 96, 120], [64, 140, 90], [200, 180, 60], [30, 30, 40]] {
        let cfg = TonedConfig::balanced(base, FACTOR).unwrap();
        let pal = TonedPalette::from_config(cfg).unwrap();
        let nudge_de: Vec<String> = [[1i16, 0, 0], [0, 1, 0], [0, 0, 1]]
            .iter()
            .map(|d| {
                let n = [
                    (base[0] as i16 + d[0]) as u8,
                    (base[1] as i16 + d[1]) as u8,
                    (base[2] as i16 + d[2]) as u8,
                ];
                format!("{:.2}", delta_e(base, n))
            })
            .collect();
        // How far the pocket's own colours already roam from the base.
        let spread = (0..pal.len()).step_by(pal.len() / 512).map(|i| delta_e(base, pal.color(i)))
            .fold(0.0f64, f64::max);
        println!(
            "base {base:?} tol={:>3}  nudge dE r/g/b = {}  |  palette already spreads to dE {:.2}",
            cfg.luma_tolerance, nudge_de.join(" / "), spread
        );
    }
}
