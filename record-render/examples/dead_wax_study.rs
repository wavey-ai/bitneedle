//! A study of a pressed dead wax: the lead-out of each profile as a pressing
//! plant cuts it, against the lead-out this build cuts.
//!
//! The proposed lead-out keeps the programme and the silent groove of the
//! render, and redraws what follows them:
//!
//! - the lead-out spiral, at least two revolutions at one constant pitch,
//!   nominally 6.4 mm a turn and never under 3.2 mm (IEC 60098: 6.4 ± 3.2 mm,
//!   at least one turn);
//! - the locked groove at the finishing-groove radius of IEC 60098 (106.4 mm
//!   across on a 12" or a 10", 97.4 mm on a 7");
//! - bare vinyl from the locked groove to the label.
//!
//! The programme may not cut below the larger of the IEC minimum inside
//! radius (the profile's payload inner radius) and the radius that holds two
//! lead-out turns at the least pitch above the lock.
//!
//! cargo run --release -p record-render --example dead_wax_study -- <out-dir>

use anyhow::{bail, Context, Result};
use record_core::{
    build_band_spiral_indices_at_angle, build_run_out_spiral_indices,
    build_silent_groove_spiral_indices, describe_record_profile, groove_angle_at_radius,
    pixels_per_mm, programme_floor_radius, SpiralFamily,
};
use record_render::{render_payload_codes_to_png, PAYLOAD_CODE_FORMAT_RGB, RECORD_WIDTH};
use std::f64::consts::PI;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const SIZE: usize = RECORD_WIDTH;
const LEAD_OUT_NOMINAL_MM: f64 = 6.4;
const LEAD_OUT_MIN_MM: f64 = 3.2;
const LEAD_OUT_MIN_TURNS: f64 = 2.0;
const SILENT_TURNS: f64 = 2.0;
const SILENT_STEP_PX: f64 = 2.0;
const VINYL: [u8; 4] = [20, 21, 22, 255];
const LABEL: [u8; 4] = [216, 206, 180, 255];
const LEAD_OUT_TONE: [u8; 4] = [168, 168, 160, 255];

/// The finishing-groove radius of IEC 60098, in millimetres.
fn iec_lock_mm(profile: &str) -> f64 {
    if profile.starts_with("single45") { 48.7 } else { 53.2 }
}

struct Proposal {
    /// The gap beside the lock is held at the nominal pitch, for etchings.
    etched: bool,
    lock: f64,
    floor: f64,
    min_pitch: f64,
    nominal_pitch: f64,
}

fn proposal(profile: &str) -> Result<Proposal> {
    let scale = pixels_per_mm(profile)?;
    let g = describe_record_profile(profile)?;
    let lock = iec_lock_mm(profile) * scale;
    let min_pitch = LEAD_OUT_MIN_MM * scale;
    // The silent groove's two turns, the raster clearance above them, and two
    // lead-out turns at the least pitch.
    let etched = std::env::var("STUDY_VARIANT").as_deref() == Ok("etched");
    let nominal_pitch = LEAD_OUT_NOMINAL_MM * scale;
    let lead_out = if etched { min_pitch + nominal_pitch } else { LEAD_OUT_MIN_TURNS * min_pitch };
    let held = lock + lead_out + SILENT_TURNS * SILENT_STEP_PX + 2.0;
    Ok(Proposal {
        etched,
        lock,
        floor: held.max(g.payload_inner_radius as f64),
        min_pitch,
        nominal_pitch,
    })
}

fn codes(bytes: usize) -> Result<Vec<u8>> {
    let fixture = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc"),
    )?;
    Ok(fixture[64..].iter().copied().cycle().take(bytes).collect())
}

struct Cut {
    rgba: Vec<u8>,
    b_value: f64,
    cut: i32,
    bytes: usize,
}

fn render(profile: &str, bytes: usize) -> Result<Cut> {
    let output = render_payload_codes_to_png(&codes(bytes)?, PAYLOAD_CODE_FORMAT_RGB, profile, 100.0, None)?;
    let rgba = image::load_from_memory(&output.png_bytes)?.to_rgba8().into_raw();
    Ok(Cut { rgba, b_value: output.payload.b_value, cut: output.payload.cut_inner_radius, bytes })
}

/// The largest payload whose cut stops at or above `floor`, or that renders at
/// all when `floor` is none.
fn fullest(profile: &str, floor: Option<f64>) -> Result<Cut> {
    let (mut low, mut high) = (20_000usize, 2_000_000usize);
    let mut best = None;
    while high - low > 2_000 {
        let middle = (low + high) / 2;
        match render(profile, middle) {
            Ok(cut) if floor.is_none_or(|f| f64::from(cut.cut) >= f) => {
                low = middle;
                best = Some(cut);
            }
            _ => high = middle,
        }
    }
    best.context("no payload fits")
}

fn radius(pixel: usize) -> f64 {
    let c = SIZE as f64 / 2.0;
    ((pixel % SIZE) as f64 + 0.5 - c).hypot((pixel / SIZE) as f64 + 0.5 - c)
}

/// The proposed lead-out over a render: the render's own lead-out removed,
/// then the silent groove, the lead-out spiral and the lock, with the
/// geometry each was cut at.
fn press(profile: &str, cut: &Cut, plan: &Proposal) -> Result<(Vec<u8>, String)> {
    let family = SpiralFamily::Archimedean;
    let mut rgba = cut.rgba.clone();
    for pixel in build_silent_groove_spiral_indices(SIZE, SIZE, cut.b_value, &family, profile, cut.cut, false)?
        .into_iter()
        .chain(build_run_out_spiral_indices(SIZE, SIZE, cut.b_value, &family, profile, Some(cut.cut), false)?)
    {
        rgba[pixel * 4..pixel * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
    }
    let start = groove_angle_at_radius(SIZE, SIZE, cut.b_value, &family, profile, f64::from(cut.cut), false)?;
    let cut_radius = f64::from(cut.cut);
    let silent_end = cut_radius - SILENT_TURNS * SILENT_STEP_PX;
    let silent = build_band_spiral_indices_at_angle(
        SIZE, SIZE, cut_radius, silent_end, SILENT_STEP_PX / (2.0 * PI), start, true,
    )?;
    // The etched variant keeps the last turn before the lock at the nominal
    // pitch; the turns above it share the rest of the room.
    let held = if plan.etched { plan.nominal_pitch } else { 0.0 };
    let room = silent_end - plan.lock - held;
    let upper_turns = if plan.etched { 1.0 } else { LEAD_OUT_MIN_TURNS };
    let turns = (room / plan.nominal_pitch).round().max(upper_turns);
    let pitch = room / turns;
    if pitch + 1e-9 < plan.min_pitch {
        bail!("{profile}: {room:.1} px leaves {turns} turns at {pitch:.1} px, under the least pitch");
    }
    let mut lead_out = build_band_spiral_indices_at_angle(
        SIZE, SIZE, silent_end, plan.lock + held + 1.0, pitch / (2.0 * PI), start + SILENT_TURNS * 2.0 * PI, true,
    )?;
    if plan.etched {
        let angle = start + (SILENT_TURNS + turns) * 2.0 * PI;
        lead_out.extend(build_band_spiral_indices_at_angle(
            SIZE, SIZE, plan.lock + held, plan.lock + 1.0, plan.nominal_pitch / (2.0 * PI), angle, true,
        )?);
    }
    let turns = turns + if plan.etched { 1.0 } else { 0.0 };
    let lock: Vec<usize> = (0..SIZE * SIZE).filter(|&p| (radius(p) - plan.lock).abs() <= 0.5).collect();
    for pixel in silent.iter().chain(&lead_out).chain(&lock) {
        if rgba[pixel * 4 + 3] == 0 {
            rgba[pixel * 4..pixel * 4 + 4].copy_from_slice(&LEAD_OUT_TONE);
        }
    }
    let scale = pixels_per_mm(profile)?;
    let label = describe_record_profile(profile)?.label_radius as f64;
    // The trailer carrier is the lead-out and the lock, at six bits a pixel.
    let mut carrier: Vec<usize> = lead_out.iter().chain(&lock).copied().collect();
    carrier.sort_unstable();
    carrier.dedup();
    let note = format!(
        "lead-out {turns} turns at {pitch:.1} px ({:.1} mm){}; trailer {} px, {} bytes; lock r {:.1} px ({:.1} mm); bare land {:.1} mm",
        pitch / scale,
        if plan.etched { format!(", the last at {:.1} mm", plan.nominal_pitch / scale) } else { String::new() },
        carrier.len(),
        carrier.len() * 6 / 8,
        plan.lock,
        plan.lock / scale,
        (plan.lock - label) / scale
    );
    Ok((rgba, note))
}

/// The current lead-out's measures, read from its pixels: the widest and the
/// narrowest gap between run-out turns along one radius, and the land inside
/// the innermost groove.
fn current_note(profile: &str, cut: &Cut) -> Result<String> {
    let scale = pixels_per_mm(profile)?;
    let label = describe_record_profile(profile)?.label_radius as f64;
    let c = SIZE as f64 / 2.0;
    // Groove crossings along the ray to the right of the centre, inside the cut.
    let mut crossings = Vec::new();
    let y = SIZE / 2;
    for x in (SIZE / 2)..SIZE {
        let r = x as f64 + 0.5 - c;
        if r > label && r < f64::from(cut.cut) - SILENT_TURNS * SILENT_STEP_PX - 1.0 && cut.rgba[(y * SIZE + x) * 4 + 3] != 0 {
            if crossings.last().is_none_or(|&last: &f64| r - last > 1.5) {
                crossings.push(r);
            }
        }
    }
    let gaps: Vec<f64> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
    if gaps.is_empty() {
        let innermost = crossings.first().copied().unwrap_or(label);
        return Ok(format!(
            "{} run-out crossing; no gap between turns; innermost groove r {innermost:.1} px; bare land {:.1} mm",
            crossings.len(),
            (innermost - label) / scale
        ));
    }
    let innermost = crossings.first().copied().unwrap_or(label);
    Ok(format!(
        "{} run-out crossings; gaps {:.1}–{:.1} px ({:.1}–{:.1} mm); innermost groove r {:.1} px; bare land {:.1} mm",
        crossings.len(),
        gaps.iter().copied().fold(f64::INFINITY, f64::min),
        gaps.iter().copied().fold(0.0, f64::max),
        gaps.iter().copied().fold(f64::INFINITY, f64::min) / scale,
        gaps.iter().copied().fold(0.0, f64::max) / scale,
        innermost,
        (innermost - label) / scale
    ))
}

fn picture(profile: &str, rgba: &[u8], path: &Path) -> Result<()> {
    let g = describe_record_profile(profile)?;
    let mut out = vec![0u8; SIZE * SIZE * 4];
    for pixel in 0..SIZE * SIZE {
        let r = radius(pixel);
        let base = if r <= g.spindle_hole_radius as f64 || r > g.outer_radius as f64 {
            [11, 11, 12, 255]
        } else if r <= g.label_radius as f64 {
            LABEL
        } else {
            VINYL
        };
        let top = &rgba[pixel * 4..pixel * 4 + 4];
        let a = top[3] as f64 / 255.0;
        for k in 0..3 {
            out[pixel * 4 + k] = (top[k] as f64 * a + base[k] as f64 * (1.0 - a)).round() as u8;
        }
        out[pixel * 4 + 3] = 255;
    }
    image::save_buffer(path, &out, SIZE as u32, SIZE as u32, image::ColorType::Rgba8)?;
    Ok(())
}

fn main() -> Result<()> {
    if let Ok(spec) = std::env::var("PROBE") {
        let (profile, bytes) = spec.split_once(':').context("PROBE=profile:bytes")?;
        probe(profile, bytes.parse()?);
        return Ok(());
    }
    let out = PathBuf::from(std::env::args().nth(1).context("Pass an output directory")?);
    std::fs::create_dir_all(&out)?;
    let mut report = String::new();
    for profile in ["single45", "ten", "lp"] {
        let plan = proposal(profile)?;
        let scale = pixels_per_mm(profile)?;
        let g = describe_record_profile(profile)?;
        let current_floor = programme_floor_radius(profile)?;
        let outer = g.payload_outer_radius as f64;
        let capacity = (outer * outer - plan.floor * plan.floor) / (outer * outer - current_floor * current_floor);
        writeln!(
            report,
            "## {profile}\n\nlabel r {} px ({:.1} mm); programme floor now {current_floor:.1} px ({:.1} mm), proposed {:.1} px ({:.1} mm); \
             IEC minimum inside r {} px; programme band at the least pitch: {:.1}% of now\n",
            g.label_radius,
            g.label_radius as f64 / scale,
            current_floor / scale,
            plan.floor,
            plan.floor / scale,
            g.payload_inner_radius,
            capacity * 100.0
        )?;
        let short = render(profile, 60_000)?;
        let full_now = fullest(profile, None)?;
        // The renderer cuts a full side down to the floor it is given.
        let key = format!("STUDY_PROGRAMME_FLOOR_{}", profile.to_uppercase());
        // SAFETY: the study is single-threaded while it sets the variable.
        unsafe { std::env::set_var(&key, format!("{}", plan.floor)) };
        let full_proposed = fullest(profile, None)?;
        unsafe { std::env::remove_var(&key) };
        for (side, now, proposed) in [("short", &short, &short), ("full", &full_now, &full_proposed)] {
            picture(profile, &now.rgba, &out.join(format!("{profile}-{side}-now.png")))?;
            let (pressed, note) = press(profile, proposed, &plan)?;
            picture(profile, &pressed, &out.join(format!("{profile}-{side}-pressed.png")))?;
            writeln!(
                report,
                "- {side} side, now ({} bytes, cut r {}): {}\n- {side} side, pressed ({} bytes, cut r {}): {note}",
                now.bytes,
                now.cut,
                current_note(profile, now)?,
                proposed.bytes,
                proposed.cut
            )?;
        }
        report.push('\n');
    }
    std::fs::write(out.join("report.md"), &report)?;
    print!("{report}");
    Ok(())
}

#[allow(dead_code)]
fn probe(profile: &str, bytes: usize) {
    match render(profile, bytes) {
        Ok(cut) => println!("probe {profile} {bytes}: ok cut r {} b {:.4}", cut.cut, cut.b_value),
        Err(error) => println!("probe {profile} {bytes}: {error:#}"),
    }
}
