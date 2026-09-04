//! A reel of constant-pitch cuts whose surface varies, not their geometry.
//!
//!     cargo run --release -p record-render --example dither_reel -- \
//!         [profile] [frames] [out-dir] [pitch-px] [payload-bytes]
//!
//! Character is held at nearly zero, so the pitch is constant: an Archimedean
//! spiral in all but name. What moves per frame is the de-moiré dither — its
//! amplitude (sheen), frequency (grain) and phase (the seed).
//!
//! That is the one axis left. Everything touching the pitch also changes how
//! much of the face is inked, so two pressings read as different qualities
//! rather than different records. The dither is exempt, and `record-core`
//! says why: it is "added to the drawn radius, never to the pitch integral".
//!
//! Drawn from the spiral mask, not from a full record render. The payload is
//! identical in every frame, so its noise is a constant that cannot tell one
//! frame from another — rendering it per frame was a hundred times the work
//! for a texture that never changes. The mask is clipped to the radius the
//! real fit stops at, which is the one thing it was getting wrong.

use anyhow::Result;
use record_core::{
    build_spiral_mask_with_family, SpiralFamily, VariPitchPlacement, VariPitchTuning,
};

const SIDE: usize = 576;

struct Rng(u64);
impl Rng {
    fn bits(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.bits() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.unit() * (hi - lo)
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let profile = args.next().unwrap_or_else(|| "ten".into());
    let frames: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(60);
    let dir = args.next().unwrap_or_else(|| "target/dither-reel".into());
    // 2.0 px, the raster floor. Below it the tracer rounds turns onto the
    // same pixels and they merge: asking 1.75 draws 2.33, asking 1.30 draws
    // 4.25. Two is the only pitch region where asked and drawn agree.
    let pitch: f64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(2.0);
    // Where the programme stops. The 140,000-byte payload at 1.7 px on the
    // ten reaches radius 190 with 62.8% of the band left, measured off the
    // real render rather than guessed.
    let ends_at: f64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(190.0);
    // `real` renders whole records through the same call CUT makes — the
    // payload laid down as RGB, the fit, the lead-out, the lot. Slower by two
    // orders of magnitude, and the only way to judge the surface as it will
    // actually be pressed.
    let real = args.next().map(|v| v == "real").unwrap_or(false);
    // Which axis this dataset moves. Everything not named is held at the
    // house cut, so a reel shows one decision and not a soup of them.
    let axis = args.next().unwrap_or_else(|| "dither".into());
    // The tone the groove is cut in, as CUT passes it. Toning costs about
    // 20% more carrier than an untoned cut — it spends bits holding each
    // pixel near the tone — so it is a capacity decision as well as a
    // colour one. Constant across a reel either way.
    let tone = args.next().unwrap_or_else(|| "#F2EEE5".into());
    let mut codes = std::fs::read(
        "goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc",
    )?;
    let full_len = codes.len();
    // The whole side unless asked otherwise. A truncated payload leaves room
    // the fit does not have in practice, so it flatters every pitch.
    if let Some(bytes) = std::env::var("PAYLOAD_BYTES").ok().and_then(|v| v.parse::<usize>().ok()) {
        codes.truncate(bytes.min(full_len));
    }
    let duration = 208.509396 * codes.len() as f64 / full_len as f64;
    std::fs::create_dir_all(&dir)?;
    println!("{profile} at {pitch} px, axis {axis}, tone {tone}, {frames} frames, real={real}");


    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut manifest =
        String::from("frame\tsheen\tgrain\tcharacter\tbands\tpitch\tseed\tcarrier\n");
    let (mut lo, mut hi) = (usize::MAX, 0usize);

    for n in 0..frames {
        let t = if frames > 1 { n as f64 / (frames - 1) as f64 } else { 0.0 };
        let (mut sheen, mut grain, mut character, mut bands, mut sep) =
            (1.0, 397.0, 0.05, 5.4, pitch);
        let seed = match axis.as_str() {
            "dither" => {
                sheen = rng.range(0.35, 0.95);
                grain = rng.range(60.0, 1400.0);
                character = 0.001;
                rng.bits() & 0x001F_FFFF_FFFF_FFFF
            }
            // Phase only: same amplitude, same frequency, different
            // rotation. Nothing about how far the groove is displaced
            // changes, so nothing about how much it inks can change either.
            // The strictest free variation there is.
            // Phase only: amplitude and frequency pinned, seed rolling.
            // Measured at 2.0 px over 24 seeds, this moves 0.222% of the ink
            // — the one axis that varies a cut without spending artwork.
            // Amplitude costs 1.341% and frequency 0.716%, so both are held.
            //
            // Spelled as vari-pitch at a depth of 0.001 rather than as a
            // dithered Archimedean, because Archimedean writes no spiral
            // segment to the descriptor and so has nowhere to record a
            // wobble a decoder would have to reproduce. A thousandth of
            // depth is no banding and needs no format change.
            "phase" => {
                sheen = 0.6;
                grain = 397.0;
                character = 0.001;
                rng.bits() & 0x001F_FFFF_FFFF_FFFF
            }
            // Frequency only: amplitude and phase pinned, grain swept. At
            // 2.0 px this moves 0.716% of the ink over 60 to 1400 — three
            // times what phase costs, still inside a per cent.
            "grain" => {
                sheen = 0.6;
                grain = 60.0 + t * 1340.0;
                character = 0.001;
                7
            }
            "seed" => rng.bits() & 0x001F_FFFF_FFFF_FFFF,
            "character" => {
                character = 0.002 + t * 0.148;
                7
            }
            "bands" => {
                bands = 4.0 + t * 16.0;
                7
            }
            "pitch" => {
                sep = pitch - 0.05 + t * 0.10;
                7
            }
            _ => rng.bits() & 0x001F_FFFF_FFFF_FFFF,
        };
        // The control: a strict Archimedean spiral and nothing else. No
        // banding, no dither, no header or trailer spiral, no lead-out, no
        // label — just the groove at one pitch, identical on every frame.
        // If frames of this differ, the difference is in the pipeline rather
        // than in the cut, and every other reel is measuring noise.
        // A strict Archimedean whose only variable is the pitch itself,
        // swept from `pitch` to `pitch + 0.20`. The arm pattern comes from
        // frac(separation) beating against the pixel grid — 2.00 is a perfect
        // lock and 2.10 is one in ten — so this is the whole of the pattern
        // variation a pure Archimedean cut has. It is paid for in seconds of
        // side: a looser pitch is fewer turns.
        if axis == "sweep" {
            sep = pitch + t * 0.20;
        }
        let fam = if axis == "control" || axis == "sweep" {
            SpiralFamily::Archimedean
        } else {
            SpiralFamily::VariPitch {
            depth: character,
            seed,
            definition: 1.0,
            sheen,
            placement: VariPitchPlacement::Even,
            fire: 0.0,
            tuning: VariPitchTuning {
                wave_one_cycles: bands,
                wave_two_cycles: 2.2,
                wave_balance: 0.62,
                dither_frequency: grain,
                aura_width: 0.3,
                fire_cycles: 14.0,
            },
            }
        };
        if real {
            // A strict Archimedean writes no spiral segment, so the family
            // key is omitted rather than set: naming vari-pitch here is what
            // made a control reel roll its seed and change every frame.
            let family = if axis == "control" || axis == "sweep" {
                String::new()
            } else {
                format!(
                    concat!(
                        r#""spiralFamily":"variPitch","grooveCharacter":{character},"#,
                        r#""grooveDefinition":1.0,"grooveSheen":{sheen},"waveOneCycles":{bands},"#,
                        r#""waveTwoCycles":2.2,"waveBalance":0.62,"ditherFrequency":{grain},"#,
                        r#""spiralSeed":{seed},"#
                    ),
                    character = character, sheen = sheen, bands = bands,
                    grain = grain, seed = seed
                )
            };
            let options = format!(
                concat!(
                    r#"{{"payloadEncoding":"rgb","dummySpiralRegions":[],"#,
                    r#""grooveToneColor":"{tone}","turnSeparationPx":{pitch},{family}"#,
                    r#""guideOutlines":false}}"#
                ),
                tone = tone, pitch = sep, family = family
            );
            let out = record_render::render_payload_codes_to_png(
                &codes, "rgb", &profile, duration, Some(&options),
            )?;
            let ink = out.payload.fit_track_pixel_count;
            lo = lo.min(ink);
            hi = hi.max(ink);
            std::fs::write(format!("{dir}/{n:04}.png"), &out.png_bytes)?;
            manifest.push_str(&format!(
                "{n}\t{sheen:.3}\t{grain:.0}\t{character:.4}\t{bands:.2}\t{sep:.4}\t{seed}\t{ink}\n"
            ));
            if n % 20 == 0 {
                println!("  {n:>4}/{frames}  sheen {sheen:.2}  grain {grain:6.0}  carrier {ink}");
            }
            continue;
        }
        let mask =
            build_spiral_mask_with_family(
                SIDE, SIDE, sep / std::f64::consts::TAU, &fam, &profile, None, None, None,
            )?;

        // Clip to where the programme actually ends. The mask runs the whole
        // band; a real cut stops and leaves the rest as deadwax.
        let centre = SIDE as f64 / 2.0;
        let mut rgba = vec![0u8; SIDE * SIDE * 4];
        let mut ink = 0usize;
        for &i in &mask.ordered_pixel_indices {
            let (x, y) = ((i % SIDE) as f64, (i / SIDE) as f64);
            let r = ((x - centre).powi(2) + (y - centre).powi(2)).sqrt();
            if r < ends_at {
                continue;
            }
            let p = i * 4;
            rgba[p] = 16;
            rgba[p + 1] = 16;
            rgba[p + 2] = 20;
            rgba[p + 3] = 255;
            ink += 1;
        }
        lo = lo.min(ink);
        hi = hi.max(ink);
        std::fs::write(
            format!("{dir}/{n:04}.png"),
            record_render::write_rgba_png(SIDE, SIDE, &rgba)?,
        )?;
        manifest.push_str(&format!(
            "{n}\t{sheen:.3}\t{grain:.0}\t{character:.4}\t{bands:.2}\t{sep:.4}\t{seed}\t{ink}\n"
        ));
    }
    std::fs::write(format!("{dir}/manifest.tsv"), manifest)?;
    println!(
        "{frames} frames in {dir}\nink {lo} – {hi} px, spread {:.2}%",
        (hi - lo) as f64 / lo.max(1) as f64 * 100.0
    );
    Ok(())
}
