//! POC: can a proof's swatches undo a global hue shift well enough to
//! recover the groove pixels?
//!
//! `cargo run -p record-proof --release --example hue_poc -- [degrees]`

use record_cut::{
    encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput, TrackInput,
};
use record_groove::{ToneOrdering, TonedConfig, TonedPalette};
use record_proof::{add_print_calibration, Corner, ProofLayout, RECORD_SIZE};
use record_render::render_chunk_stream_to_png;

fn hue_shift(rgb: [u8; 3], degrees: f64) -> [u8; 3] {
    // Rotate hue in YIQ space (a linear-in-RGB rotation, then quantise).
    let [r, g, b] = rgb.map(f64::from);
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let i = 0.596 * r - 0.274 * g - 0.322 * b;
    let q = 0.211 * r - 0.523 * g + 0.312 * b;
    let (s, c) = degrees.to_radians().sin_cos();
    let (i, q) = (i * c - q * s, i * s + q * c);
    let r = y + 0.956 * i + 0.621 * q;
    let g = y - 0.272 * i - 0.647 * q;
    let b = y - 1.106 * i + 1.703 * q;
    [r, g, b].map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// Least-squares fit of expected = A·observed + t per channel (3×4 affine).
fn fit_affine(pairs: &[([f64; 3], [f64; 3])]) -> [[f64; 4]; 3] {
    // Normal equations: (XᵀX) w = Xᵀy, X rows = [o_r, o_g, o_b, 1].
    let mut xtx = [[0.0f64; 4]; 4];
    for (a, row) in xtx.iter_mut().enumerate() {
        row[a] = 1e-6; // ridge, keeps degenerate swatch sets solvable
    }
    let mut xty = [[0.0f64; 4]; 3];
    for (o, e) in pairs {
        let x = [o[0], o[1], o[2], 1.0];
        for a in 0..4 {
            for b in 0..4 {
                xtx[a][b] += x[a] * x[b];
            }
            for ch in 0..3 {
                xty[ch][a] += x[a] * e[ch];
            }
        }
    }
    let mut out = [[0.0; 4]; 3];
    for ch in 0..3 {
        // Gaussian elimination on a copy.
        let mut m = [[0.0f64; 5]; 4];
        for a in 0..4 {
            m[a][..4].copy_from_slice(&xtx[a]);
            m[a][4] = xty[ch][a];
        }
        for col in 0..4 {
            let pivot = (col..4).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs())).unwrap();
            m.swap(col, pivot);
            for row in 0..4 {
                if row != col {
                    let f = m[row][col] / m[col][col];
                    for k in col..5 {
                        m[row][k] -= f * m[col][k];
                    }
                }
            }
        }
        for a in 0..4 {
            out[ch][a] = m[a][4] / m[a][a];
        }
    }
    out
}

fn apply(m: &[[f64; 4]; 3], rgb: [u8; 3]) -> [u8; 3] {
    let o = rgb.map(f64::from);
    let mut out = [0u8; 3];
    for ch in 0..3 {
        let v = m[ch][0] * o[0] + m[ch][1] * o[1] + m[ch][2] * o[2] + m[ch][3];
        out[ch] = v.round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn px(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * RECORD_SIZE + x) * 4;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn main() {
    let degrees: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(4.0);

    // 1. Render a toned record and add the proof corners.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../goldenfiles/records");
    let id = "lori-asha-westside-single45-hq";
    let payload = std::fs::read(root.join(id).join(format!("{id}.ecdc"))).unwrap();
    let input = RecordStreamInput {
        payload_descriptors: vec![PayloadDescriptorInput::from_container("ECDC")],
        tracks: vec![TrackInput { title: "Test".into(), first_revolution_index: None, revolution_count: None }],
        track_gaps: vec![],
    };
    let stream = encode_record_stream(&input, &[PayloadEntryInput { payload_descriptor_index: 0, bytes: payload }]).unwrap();
    let options = serde_json::json!({ "grooveToneColor": "#FFC0CB" }).to_string();
    let rendered = render_chunk_stream_to_png(&stream, "single45", 208.509396, Some(&options)).unwrap();
    let proof = add_print_calibration(&rendered.png_bytes).unwrap();
    let layout = ProofLayout::for_descriptor(&rendered.descriptor).unwrap();
    let clean = image::load_from_memory(&proof.png).unwrap().to_rgba8().into_raw();

    // 2. Hue-shift every opaque pixel (disc and corners alike).
    let mut shifted = clean.clone();
    for p in shifted.chunks_exact_mut(4) {
        if p[3] != 0 {
            let c = hue_shift([p[0], p[1], p[2]], degrees);
            p[..3].copy_from_slice(&c);
        }
    }

    // 3. Fit the correction from the swatches only (expected from layout, observed from image).
    let mut pairs = Vec::new();
    for corner in Corner::SWATCH_CORNERS {
        for cell in &layout.cells {
            let pixels = layout.cell_pixels(corner, cell);
            let mut sum = [0.0f64; 3];
            for &(x, y) in &pixels {
                let p = px(&shifted, x, y);
                for ch in 0..3 {
                    sum[ch] += f64::from(p[ch]);
                }
            }
            let n = pixels.len() as f64;
            pairs.push((sum.map(|v| v / n), cell.color.map(f64::from)));
        }
    }
    let m = fit_affine(&pairs);

    // 4. Correct the disc and compare with the clean pixels.
    let (mut total, mut exact_before, mut exact_after, mut within1) = (0usize, 0usize, 0usize, 0usize);
    let mut corrected = shifted.clone();
    for (i, p) in corrected.chunks_exact_mut(4).enumerate() {
        if p[3] == 0 {
            continue;
        }
        let orig = [clean[i * 4], clean[i * 4 + 1], clean[i * 4 + 2]];
        total += 1;
        if [p[0], p[1], p[2]] == orig {
            exact_before += 1;
        }
        let c = apply(&m, [p[0], p[1], p[2]]);
        p[..3].copy_from_slice(&c);
        if c == orig {
            exact_after += 1;
        }
        if c.iter().zip(orig.iter()).all(|(&a, &b)| (i16::from(a) - i16::from(b)).abs() <= 1) {
            within1 += 1;
        }
    }
    println!("hue shift {degrees:.1}°, fit from {} swatch pairs", pairs.len());
    println!("affine fit: {:?}", m.map(|r| r.map(|v| (v * 1000.0).round() / 1000.0)));
    println!(
        "opaque pixels {total}: exact before {:.2}%, exact after {:.2}%, within ±1 after {:.2}%",
        100.0 * exact_before as f64 / total as f64,
        100.0 * exact_after as f64 / total as f64,
        100.0 * within1 as f64 / total as f64
    );

    let encode = |rgba: &[u8]| {
        use image::ImageEncoder;
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(rgba, RECORD_SIZE as u32, RECORD_SIZE as u32, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    };
    for (name, img) in [("shifted", &shifted), ("corrected", &corrected)] {
        match record_decode::decode_record_png_to_chunk_stream(&encode(img)) {
            Ok((_, s)) if s.bytes == stream => println!("{name}: decodes to the ORIGINAL stream"),
            Ok(_) => println!("{name}: decodes, but to a DIFFERENT stream"),
            Err(e) => println!("{name}: decode FAILED: {}", e.to_string().lines().next().unwrap_or("")),
        }
    }

    // 5. bpp sweep with noise: current palette selection vs a max-separation
    //    selection, hue shift + Gaussian noise, affine fit from the palette,
    //    nearest-palette snap.
    let base = [255u8, 192, 203];
    let tol = 64u8;
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut gauss = move || -> f64 {
        let mut u = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        let (u1, u2) = (u().max(1e-12), u());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    };
    let dist = |a: [u8; 3], b: [u8; 3]| -> i64 {
        a.iter().zip(b.iter()).map(|(&x, &y)| (i64::from(x) - i64::from(y)).pow(2)).sum()
    };
    let candidates = TonedPalette::candidates(base, tol);
    println!(
        "\nbpp sweep: base #FFC0CB tol {tol} ({} iso-luma candidates), hue {degrees:.1}° + noise σ, snap after affine fit",
        candidates.len()
    );
    println!("  {:<10} {:>4} {:>7} | {:>7} {:>7} {:>7}", "selection", "bpp", "min-sep", "σ=0", "σ=1", "σ=2");
    for bpp in [2u32, 4, 6, 8, 10] {
        let n = 1usize << bpp;
        let current: Vec<[u8; 3]> = TonedPalette::from_config(TonedConfig {
            base,
            luma_tolerance: tol,
            bits_per_pixel: bpp,
            ordering: ToneOrdering::ChromaProximity,
        })
        .map(|p| (0..p.len()).map(|i| p.color(i)).collect())
        .unwrap();
        // Greedy farthest-point over a strided subset of the candidates.
        let pool: Vec<[u8; 3]> = candidates.iter().copied().step_by((candidates.len() / 20_000).max(1)).collect();
        let mut spread = vec![base];
        let mut best = vec![i64::MAX; pool.len()];
        while spread.len() < n {
            let last = *spread.last().unwrap();
            for (i, &c) in pool.iter().enumerate() {
                best[i] = best[i].min(dist(c, last));
            }
            let far = (0..pool.len()).max_by_key(|&i| best[i]).unwrap();
            spread.push(pool[far]);
        }
        for (name, colors) in [("current", &current), ("spread", &spread)] {
            let mut min_sep = i64::MAX;
            for (i, &a) in colors.iter().enumerate() {
                for &b in &colors[i + 1..] {
                    min_sep = min_sep.min(dist(a, b));
                }
            }
            let mut cols = String::new();
            for sigma in [0.0, 1.0, 2.0] {
                let noisy = |c: [u8; 3], g: &mut dyn FnMut() -> f64| -> [u8; 3] {
                    hue_shift(c, degrees).map(|v| (f64::from(v) + sigma * g()).round().clamp(0.0, 255.0) as u8)
                };
                let pairs: Vec<_> = colors.iter().map(|&c| (noisy(c, &mut gauss).map(f64::from), c.map(f64::from))).collect();
                let m = fit_affine(&pairs);
                let trials = 20;
                let mut ok = 0usize;
                for (idx, &c) in colors.iter().enumerate() {
                    for _ in 0..trials {
                        let corrected = apply(&m, noisy(c, &mut gauss));
                        let nearest = (0..colors.len()).min_by_key(|&j| dist(colors[j], corrected)).unwrap();
                        ok += usize::from(nearest == idx);
                    }
                }
                cols += &format!(" {:>6.1}%", 100.0 * ok as f64 / (colors.len() * trials) as f64);
            }
            println!("  {name:<10} {bpp:>4} {:>7.1} |{cols}", (min_sep as f64).sqrt());
        }
    }
}
