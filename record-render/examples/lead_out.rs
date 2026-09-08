//! The lead-out drawn on its own: the run-out turns, the locked groove that
//! they descend into, and a label.
//!
//! The band is one groove. The lathe feeds the head in at a coarse pitch after
//! the programme ends. Part way through the last revolution the feed switches
//! off, the groove stops descending, becomes a circle, and returns to the point
//! at which the feed stopped. The three concentric lines that the eye reads are
//! one continuous cut at three radii.
//!
//! The band is therefore one carrier: one ordered pixel sequence, entered at
//! the outermost turn and cyclic at the innermost turn. Its capacity follows
//! from the geometry alone, and the geometry follows from the scale of the
//! profile.
//!
//! cargo run -p record-render --example lead_out -- [profile] [cut_inner_radius] [out.png]

use anyhow::Result;
use record_core::{
    describe_record_profile, lead_out_geometry_with_extent, pixels_per_mm, LeadOutExtent,
};
use std::f64::consts::PI;

const CANVAS: usize = 576;





fn js_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

/// One lead-out, as an ordered pixel sequence.
///
/// Returns the sequence and the index the run-out gives way to the lock at,
/// which is also the index the sequence wraps back to: walking off the end
/// lands there, not at zero. The run-out is walked once; the lock is walked
/// forever.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let profile = args.get(1).cloned().unwrap_or_else(|| "lp".to_string());
    let cut_inner_radius: Option<i32> = args.get(2).and_then(|v| v.parse().ok());
    let out_path = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| format!("lead-out-{profile}.png"));

    let record = describe_record_profile(&profile)?;
    let scale = pixels_per_mm(&profile)?;
    let extent = match std::env::var("EXTENT").as_deref() {
        Ok("extended") => LeadOutExtent::Extended,
        Ok("wide") => LeadOutExtent::Wide,
        Ok("extrawide") => LeadOutExtent::ExtraWide,
        _ => LeadOutExtent::Compact,
    };
    let geometry = lead_out_geometry_with_extent(&profile, cut_inner_radius, extent)?;
    let (indices, lock_start) = record_core::build_lead_out_indices_with_extent(
        CANVAS,
        CANVAS,
        &profile,
        cut_inner_radius,
        extent,
    )?;

    let mut data = vec![0_u8; CANVAS * CANVAS * 4];
    let center = CANVAS as f64 / 2.0;

    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let dx = x as f64 - center;
            let dy = y as f64 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let offset = (y * CANVAS + x) * 4;

            let (r, g, b) = if distance <= record.label_radius as f64 {
                (0xD8, 0xCE, 0xB4)
            } else if (distance - record.payload_inner_radius as f64).abs() < 0.5 {
                // Where the programme band used to stop, for reference only.
                (0x33, 0x2A, 0x22)
            } else {
                (0x12, 0x12, 0x14)
            };

            data[offset] = r;
            data[offset + 1] = g;
            data[offset + 2] = b;
            data[offset + 3] = 0xFF;
        }
    }

    if std::env::var("RUNIN").as_deref() == Ok("1") {
        // The lead-out does not start, it continues: above the entry radius
        // the deadwax has been running at the lathe's own feed since the
        // programme stopped. Drawn here for half a turn so the outermost line
        // arrives from somewhere instead of terminating in mid-air.
        let feed = record_core::deadwax_spiral_pitch(&profile)?;
        let mut back = 0.0_f64;
        while back < PI {
            let radius = geometry.entry_radius + feed * back;
            let angle = record_core::DEFAULT_START_ANGLE + back;
            let x = js_round(center + radius * angle.cos());
            let y = js_round(center - radius * angle.sin());
            if x >= 0 && x < CANVAS as i32 && y >= 0 && y < CANVAS as i32 {
                let offset = (y as usize * CANVAS + x as usize) * 4;
                data[offset] = 0x8C;
                data[offset + 1] = 0x8C;
                data[offset + 2] = 0x8C;
                data[offset + 3] = 0xFF;
            }
            back += 1.0 / radius.max(1e-6);
        }
    }

    for (position, &index) in indices.iter().enumerate() {
        let tone = if position < lock_start { 0xB4 } else { 0xE6 };
        let offset = index * 4;
        data[offset] = tone;
        data[offset + 1] = tone;
        data[offset + 2] = tone;
        data[offset + 3] = 0xFF;
    }

    image::RgbaImage::from_raw(CANVAS as u32, CANVAS as u32, data.clone())
        .expect("raster dimensions match the buffer")
        .save(&out_path)?;

    // The join is a few pixels of detail on a 576 canvas, which is true to
    // the record and useless to look at. Blow it up on its own.
    let window = 128_usize;
    let scale_up = 4_usize;
    let left = (CANVAS / 2) as i64 - (window / 2) as i64;
    let top = (CANVAS as f64 / 2.0 - geometry.entry_radius) as i64 - (window / 3) as i64;
    let side = window * scale_up;
    let mut detail = vec![0_u8; side * side * 4];

    for y in 0..side {
        for x in 0..side {
            let sx = left + (x / scale_up) as i64;
            let sy = top + (y / scale_up) as i64;
            let dst = (y * side + x) * 4;

            if sx < 0 || sy < 0 || sx >= CANVAS as i64 || sy >= CANVAS as i64 {
                detail[dst + 3] = 0xFF;
                continue;
            }

            let src = (sy as usize * CANVAS + sx as usize) * 4;
            detail[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
        }
    }

    let detail_path = out_path.replace(".png", "-join.png");
    image::RgbaImage::from_raw(side as u32, side as u32, detail)
        .expect("detail dimensions match the buffer")
        .save(&detail_path)?;

    let mm = |px: f64| px / scale;
    let turns = geometry.turns;

    println!("profile           {profile}  ({scale:.4} px/mm)");
    println!("label radius      {:3} px", record.label_radius);
    println!(
        "lock groove       {:5.1} px   label + {:.2} mm",
        geometry.lock_radius,
        mm(geometry.lock_radius - record.label_radius as f64)
    );
    println!(
        "run-out entry     {:5.1} px   label + {:.2} mm   ({turns:.0} turns, inner gap {:.1} px)",
        geometry.entry_radius,
        mm(geometry.entry_radius - record.label_radius as f64),
        geometry.inner_gap,
    );
    println!(
        "gaps px           {}",
        geometry
            .gaps()
            .iter()
            .map(|gap| format!("{gap:.1}"))
            .collect::<Vec<_>>()
            .join("  ")
    );
    println!(
        "payload inner     {:3} px   (the run-out now reaches {:.1} px past it)",
        record.payload_inner_radius,
        geometry.entry_radius - record.payload_inner_radius as f64
    );
    println!();
    println!("run-out           {lock_start} px");
    println!("locked groove     {} px  (cyclic)", indices.len() - lock_start);
    println!(
        "carrier capacity  {} px = {} bytes at two pixels a byte",
        indices.len(),
        indices.len() / 2
    );
    println!("\nwrote {out_path}");
    println!("wrote {detail_path}  ({window} px window at {scale_up}x, on the join)");

    Ok(())
}
