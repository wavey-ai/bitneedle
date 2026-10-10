//! A measure of the pressed dead wax that lead-out geometry revision 4 cuts.
//!
//! For each profile the study renders a short side (60,000 payload bytes) and
//! the largest payload that the profile holds. It reads each record back and
//! writes a picture of it. The report gives the lead-out gaps along one
//! radius, the locked-groove radius, the bare land inside the locked groove,
//! and the trailer capacity.
//!
//! cargo run --release -p record-render --example dead_wax_study -- <out-dir>

use anyhow::{ensure, Context, Result};
use record_core::{
    build_run_out_spiral_indices_at, describe_record_profile, lead_out_geometry_with_extent_at,
    pixels_per_mm, programme_floor_radius_at, LeadOutExtent, SpiralFamily,
};
use record_render::{render_payload_codes_to_png, PAYLOAD_CODE_FORMAT_RGB, RECORD_WIDTH};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const SIZE: usize = RECORD_WIDTH;
const SHORT_SIDE_BYTES: usize = 60_000;
const VINYL: [u8; 4] = [20, 21, 22, 255];
const LABEL: [u8; 4] = [216, 206, 180, 255];

/// The full side of each profile at revision 3, measured with this example
/// at f4e03db. The ratio in the report compares revision 4 with these.
fn revision_3_full_side(profile: &str) -> usize {
    match profile {
        "single45" => 426_054,
        "ten" => 513_066,
        _ => 561_406,
    }
}

fn codes(bytes: usize) -> Result<Vec<u8>> {
    let fixture = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc"),
    )?;
    // Without the stream header, the renderer takes the bytes as a code block
    // of any length.
    Ok(fixture[64..].iter().copied().cycle().take(bytes).collect())
}

struct Cut {
    png: Vec<u8>,
    rgba: Vec<u8>,
    b_value: f64,
    cut: i32,
    bytes: usize,
}

fn render(profile: &str, bytes: usize) -> Result<Cut> {
    let output = render_payload_codes_to_png(
        &codes(bytes)?,
        PAYLOAD_CODE_FORMAT_RGB,
        profile,
        100.0,
        None,
    )?;
    let rgba = image::load_from_memory(&output.png_bytes)?
        .to_rgba8()
        .into_raw();
    Ok(Cut {
        png: output.png_bytes,
        rgba,
        b_value: output.payload.b_value,
        cut: output.payload.cut_inner_radius,
        bytes,
    })
}

/// The largest payload that renders, to the nearest 1,000 bytes.
fn fullest(profile: &str) -> Result<Cut> {
    let (mut low, mut high) = (20_000usize, 2_000_000usize);
    let mut best = None;
    while high - low > 1_000 {
        let middle = (low + high) / 2;
        match render(profile, middle) {
            Ok(cut) => {
                low = middle;
                best = Some(cut);
            }
            Err(_) => high = middle,
        }
    }
    best.context("no payload fits")
}

fn radius(pixel: usize) -> f64 {
    let c = SIZE as f64 / 2.0;
    ((pixel % SIZE) as f64 + 0.5 - c).hypot((pixel / SIZE) as f64 + 0.5 - c)
}

/// The groove crossings along the ray to the right of the centre, from the
/// label out to the cut radius.
fn crossings(profile: &str, cut: &Cut) -> Result<Vec<f64>> {
    let label = describe_record_profile(profile)?.label_radius as f64;
    let c = SIZE as f64 / 2.0;
    let y = SIZE / 2;
    let mut found: Vec<f64> = Vec::new();
    for x in (SIZE / 2)..SIZE {
        let r = x as f64 + 0.5 - c;
        if r > label
            && r < f64::from(cut.cut)
            && cut.rgba[(y * SIZE + x) * 4 + 3] != 0
            && found.last().is_none_or(|&last| r - last > 1.5)
        {
            found.push(r);
        }
    }
    Ok(found)
}

fn note(profile: &str, cut: &Cut) -> Result<String> {
    // The descriptor overflows into the trailer, so a descriptor that reads
    // back proves the trailer that the reader traces at the record's revision.
    let (_, descriptor) = record_decode::decode_record_descriptor_from_png(&cut.png, None)
        .with_context(|| format!("{profile}: a {} byte cut does not read back", cut.bytes))?;
    let revision = descriptor.lead_out_geometry_revision;
    ensure!(
        descriptor.stream_byte_length == cut.bytes,
        "{profile}: a {} byte cut reads back as {} bytes",
        cut.bytes,
        descriptor.stream_byte_length
    );
    let scale = pixels_per_mm(profile)?;
    let label = describe_record_profile(profile)?.label_radius as f64;
    let lead_out =
        lead_out_geometry_with_extent_at(profile, Some(cut.cut), LeadOutExtent::Fill, revision)?;
    let lock = lead_out.lock_radius;
    let trailer = build_run_out_spiral_indices_at(
        SIZE,
        SIZE,
        cut.b_value,
        &SpiralFamily::Archimedean,
        profile,
        Some(cut.cut),
        false,
        revision,
    )?
    .len();
    // The rings that one radius crosses from the label to the cut: the lock,
    // the run-out turns and the silent groove turns.
    let rings = crossings(profile, cut)?.len();
    Ok(format!(
        "revision {revision}, cut r {} px; run-out {} turns at {:.1} mm from r {:.1} px; \
         {rings} rings on one radius; lock r {lock:.1} px ({:.1} mm); bare land {:.1} mm; \
         trailer {trailer} px, {} bytes",
        cut.cut,
        lead_out.turns,
        lead_out.turn_separation / scale,
        lead_out.entry_radius,
        lock / scale,
        (lock - label) / scale,
        trailer * 6 / 8
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
    image::save_buffer(
        path,
        &out,
        SIZE as u32,
        SIZE as u32,
        image::ColorType::Rgba8,
    )?;
    Ok(())
}

fn main() -> Result<()> {
    if let Ok(spec) = std::env::var("PROBE") {
        let (profile, bytes) = spec.split_once(':').context("PROBE=profile:bytes")?;
        let cut = render(profile, bytes.parse()?)?;
        println!("{}", note(profile, &cut)?);
        return Ok(());
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Pass an output directory")?,
    );
    std::fs::create_dir_all(&out)?;
    let mut report = String::new();
    for profile in ["single45", "ten", "lp"] {
        let scale = pixels_per_mm(profile)?;
        let floor = programme_floor_radius_at(profile, record_core::LEAD_OUT_REVISION)?;
        let short = render(profile, SHORT_SIDE_BYTES)?;
        let full = fullest(profile)?;
        writeln!(
            report,
            "## {profile}\n\nprogramme floor r {floor:.1} px ({:.1} mm); full side {} bytes, \
             {:.1}% of revision 3\n",
            floor / scale,
            full.bytes,
            100.0 * full.bytes as f64 / revision_3_full_side(profile) as f64
        )?;
        for (side, cut) in [("short", &short), ("full", &full)] {
            picture(
                profile,
                &cut.rgba,
                &out.join(format!("{profile}-{side}.png")),
            )?;
            writeln!(
                report,
                "- {side} side, {} bytes: {}",
                cut.bytes,
                note(profile, cut)?
            )?;
        }
        report.push('\n');
    }
    std::fs::write(out.join("report.md"), &report)?;
    print!("{report}");
    Ok(())
}
