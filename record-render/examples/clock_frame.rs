//! One whole record cut with a clockface: sixteen pockets of a hue wheel,
//! spun, blended, on the full Westside LP payload. Prints how long the cut
//! and the decode took and writes the PNG to target/fixtures.
//!
//!     cargo run --release -p record-render --example clock_frame -- [slots] [rotation°] [blend]
use anyhow::Result;
use record_cut::{
    encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput,
    TrackGapInput, TrackInput,
};
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let slots: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(16);
    let rotation: f64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let blend: bool = args.next().map(|v| v != "hard").unwrap_or(true);

    // The ECDC wrapped as a BRS1 stream the way CUT wraps it, with a track
    // gap between two sides so the per-pocket gap tones show.
    let ecdc = std::fs::read(
        "goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc",
    )?;
    let side_a = ecdc.len() * 45 / 100;
    let gap_end = side_a + ecdc.len() * 3 / 100;
    let input = RecordStreamInput {
        payload_descriptors: vec![PayloadDescriptorInput::from_container("ECDC")],
        tracks: vec![
            TrackInput { title: "Side A".into(), first_revolution_index: Some(0), revolution_count: Some(1) },
            TrackInput { title: "Side B".into(), first_revolution_index: Some(2), revolution_count: Some(1) },
        ],
        track_gaps: vec![TrackGapInput { first_revolution_index: 1, revolution_count: 1, after_track_index: 0 }],
    };
    let entries: Vec<PayloadEntryInput> = [&ecdc[..side_a], &ecdc[side_a..gap_end], &ecdc[gap_end..]]
        .into_iter()
        .map(|bytes| PayloadEntryInput { payload_descriptor_index: 0, bytes: bytes.to_vec() })
        .collect();
    let codes = encode_record_stream(&input, &entries)?;
    let wheel: Vec<String> = (0..slots)
        .map(|k| {
            let t = k as f64 / slots as f64 * std::f64::consts::TAU;
            format!(
                "#{:02X}{:02X}{:02X}",
                (150.0 + 90.0 * t.cos()) as u8,
                (150.0 + 90.0 * (t + 2.094).cos()) as u8,
                (150.0 + 90.0 * (t + 4.189).cos()) as u8
            )
        })
        .collect();
    let options = serde_json::json!({
        "grooveToneSlots": wheel,
        "grooveToneRotationDegrees": rotation,
        "grooveToneBlend": blend,
    })
    .to_string();

    let t = Instant::now();
    let output = record_render::render_payload_codes_to_png(&codes, "rgb", "lp", 208.5, Some(&options))?;
    let cut = t.elapsed();
    std::fs::create_dir_all("target/fixtures")?;
    let path = format!("target/fixtures/clock-{slots}-{rotation}-{}.png", if blend { "blend" } else { "hard" });
    std::fs::write(&path, &output.png_bytes)?;

    let t = Instant::now();
    let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
        &output.png_bytes,
        "lp",
        Some(codes.len()),
    )?;
    let decode = t.elapsed();
    assert_eq!(decoded.bytes, codes, "round trip");
    println!(
        "{slots} slots, {rotation}°, {}: cut {cut:.2?}, decode {decode:.2?}, {} px, {} bytes PNG -> {path}",
        if blend { "blend" } else { "hard" },
        output.payload.filtered_pixel_count,
        output.png_bytes.len()
    );
    Ok(())
}
