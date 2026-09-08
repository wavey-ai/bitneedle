//! What a record costs in each format it can leave in, and the proof that
//! leaving in one is not a change to the record.
//!
//! One whole LP is cut from the Westside payload, written in every format
//! this build carries, read back, and decoded. The table is size, encode
//! time, decode time, and whether the groove came back byte for byte.
//!
//!     cargo run --release -p record-render --features all-formats \
//!       --example export_formats -- [toned|rgb]
//!
//! A build with the default features writes PNG alone. The format features
//! keep the other encoders out of a build that leaves them off, so a phone that
//! exports PNG links no TIFF encoder.

use anyhow::Result;
use record_cut::{
    encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput, TrackInput,
};
use record_render::export::{read_rgba, write_rgba, RecordImageFormat};
use std::time::Instant;

fn main() -> Result<()> {
    let toned = std::env::args().nth(1).map(|arg| arg != "rgb").unwrap_or(true);

    let ecdc = std::fs::read(
        "goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.ecdc",
    )?;
    let input = RecordStreamInput {
        payload_descriptors: vec![PayloadDescriptorInput::from_container("ECDC")],
        tracks: vec![TrackInput {
            title: "Side A".into(),
            first_revolution_index: None,
            revolution_count: None,
        }],
        track_gaps: vec![],
    };
    let entries = vec![PayloadEntryInput {
        payload_descriptor_index: 0,
        bytes: ecdc.clone(),
    }];
    let codes = encode_record_stream(&input, &entries)?;

    // A toned cut is the hard case: every pocket carries its own palette of
    // a million iso-luma colours, packed at twenty bits a pixel, so a
    // format that moves one channel of one pixel by one step loses the
    // record. An RGB cut carries three bytes in every pixel and is no
    // gentler, only faster to make.
    let options = toned.then(|| {
        let wheel: Vec<String> = (0..16)
            .map(|k| {
                let t = k as f64 / 16.0 * std::f64::consts::TAU;
                format!(
                    "#{:02X}{:02X}{:02X}",
                    (150.0 + 90.0 * t.cos()) as u8,
                    (150.0 + 90.0 * (t + 2.094).cos()) as u8,
                    (150.0 + 90.0 * (t + 4.189).cos()) as u8
                )
            })
            .collect();
        serde_json::json!({
            "grooveToneSlots": wheel,
            "grooveToneRings": [8, 8],
            "grooveToneRotationDegrees": [0.0, 33.0],
            "grooveToneBlend": true,
        })
        .to_string()
    });

    let cut = Instant::now();
    let output =
        record_render::render_payload_codes_to_png(&codes, "rgb", "lp", 208.5, options.as_deref())?;
    println!(
        "{} LP: {} bytes of payload, cut in {:.2?}",
        if toned { "toned" } else { "rgb" },
        codes.len(),
        cut.elapsed()
    );

    let (width, height, pixels) = read_rgba(&output.png_bytes)?;
    std::fs::create_dir_all("target/fixtures")?;
    println!();
    println!("format    size        write     read      groove");

    for format in RecordImageFormat::available() {
        let at = Instant::now();
        let written = write_rgba(format, width, height, &pixels)?;
        let wrote = at.elapsed();

        let at = Instant::now();
        let (back_width, back_height, back) = read_rgba(&written)?;
        let read = at.elapsed();

        let path = format!("target/fixtures/export-lp.{}", format.extension());
        std::fs::write(&path, &written)?;

        let same = (back_width, back_height) == (width, height) && back == pixels;
        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &written,
            "lp",
            Some(codes.len()),
        )
        .map(|decoded| decoded.bytes == codes)
        .unwrap_or(false);

        println!(
            "{:<9} {:>9}   {:>7.1?}   {:>7.1?}   {}",
            format.id(),
            format!("{:.1} MB", written.len() as f64 / 1_048_576.0),
            wrote,
            read,
            match (same, decoded) {
                (true, true) => "exact, reads",
                (true, false) => "exact, WILL NOT READ",
                (false, _) => "PIXELS CHANGED",
            }
        );
    }

    Ok(())
}
