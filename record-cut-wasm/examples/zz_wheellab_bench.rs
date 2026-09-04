// TEMPORARY stage-timing bench for the wheel-lab cut path. Delete after use.
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let ecdc = std::fs::read(
        "goldenfiles/records/lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.ecdc",
    )?;
    let slots: Vec<String> = [
        "#80701F", "#C81E28", "#B53E2F", "#BC3932", "#BA3B30", "#AD4330", "#955C25",
        "#6E7A22", "#C81E28", "#C81F28", "#558924", "#628024", "#8F6222", "#A7472E",
        "#B93C31", "#C53531", "#CC3132", "#CB3131", "#C43631", "#B63E31", "#A44B2D",
        "#876920", "#687C23", "#568824", "#499024", "#449324", "#449324", "#4B8E25",
        "#598624", "#6E7A23",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let rings = vec![12u32, 18u32];
    let spin = 37.0f64;
    const GEAR: f64 = 0.6180339887498949;
    let rots: Vec<f64> = rings
        .iter()
        .enumerate()
        .map(|(r, _)| spin * GEAR.powi((rings.len() - 1 - r) as i32))
        .collect();
    let options = serde_json::json!({
        "payloadEncoding": "rgb",
        "grooveToneSlots": slots,
        "grooveToneRings": rings,
        "grooveToneRotationDegrees": rots,
        "grooveToneBlend": true,
        "fastFit": false,
    });
    let options_json = serde_json::to_string(&options)?;

    // Same stream wrapping the facade does.
    let t = Instant::now();
    let stream = record_cut::encode_record_stream(
        &record_cut::RecordStreamInput {
            payload_descriptors: vec![
                record_cut::PayloadDescriptorInput::from_container("ECDC"),
            ],
            tracks: vec![record_cut::TrackInput {
                title: "Track 1".into(),
                first_revolution_index: Some(0),
                revolution_count: Some(1),
            }],
            track_gaps: vec![],
        },
        &[record_cut::PayloadEntryInput::already_chunked(0, ecdc.clone())],
    )?;
    println!("  encode_record_stream: {:.2}s ({} bytes)", t.elapsed().as_secs_f64(), stream.len());

    // Warm caches once so the second pass measures steady state.
    let _ = record_render::render_chunk_stream_to_png(&stream, "single45", 208.5, Some(&options_json))?;

    let t = Instant::now();
    let rendered =
        record_render::render_chunk_stream_to_png(&stream, "single45", 208.5, Some(&options_json))?;
    println!("  render_chunk_stream_to_png (warm): {:.2}s png={}KB", t.elapsed().as_secs_f64(), rendered.png_bytes.len() / 1024);

    let t = Instant::now();
    let (_, descriptor) = record_decode::decode_record_descriptor_from_png(
        &rendered.png_bytes,
        Some("single45"),
    )?;
    println!("  decode descriptor: {:.2}s", t.elapsed().as_secs_f64());
    let _ = descriptor;

    let t = Instant::now();
    let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
        &rendered.png_bytes,
        "single45",
        Some(stream.len()),
    )?;
    // PNG encoder levels on the actual noisy groove pixels.
    {
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        use image::{ExtendedColorType, ImageEncoder};
        let img = image::load_from_memory(&rendered.png_bytes)?.to_rgba8();
        let (w, h) = (img.width(), img.height());
        let raw = img.as_raw();
        for (name, comp, filter) in [
            ("best+adaptive", CompressionType::Best, FilterType::Adaptive),
            ("default+adaptive", CompressionType::Default, FilterType::Adaptive),
            ("fast+adaptive", CompressionType::Fast, FilterType::Adaptive),
            ("default+sub", CompressionType::Default, FilterType::Sub),
        ] {
            let t = Instant::now();
            let mut out = Vec::new();
            PngEncoder::new_with_quality(&mut out, comp, filter)
                .write_image(raw, w, h, ExtendedColorType::Rgba8)?;
            let dt = t.elapsed().as_secs_f64();
            // Byte-identical pixels back?
            let back = image::load_from_memory(&out)?.to_rgba8();
            println!("  png {name}: {:.2}s {}KB identical={}", dt, out.len() / 1024, back.as_raw() == raw);
        }
    }
    Ok(())
}
