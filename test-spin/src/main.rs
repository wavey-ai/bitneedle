//! record-test — verbose Bitneedle record PNG decoder / layout dumper.
//!
//! Usage:
//!   record-test <record.png> [bundle.json]
//!
//! Decodes a record PNG using the exact native crates the browser wasm is built
//! from (`record-decode`, `record-core`, `encodec-rs`) and prints the full
//! layout at every level — descriptor header, BCS2 chunk stream, the reassembled
//! payload, the BPLP length-prefixed entry table, and each entry's ECDC header +
//! chunk table — so we can see precisely where the bytes stop being valid.
//!
//! No audio is decoded (no ONNX / soundkit): the point is to find out whether we
//! even recover valid ECDC bytes for each track.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use encodec_rs::binary::{read_chunk_payload, read_ecdc_header};
use encodec_rs::format::{segment_starts, EcdcMetadata};

fn main() {
    if let Err(err) = run() {
        eprintln!("\n!! record-test failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let png_path = args
        .next()
        .context("usage: record-test <record.png> [bundle.json]")?;
    let bundle_path = args.next();

    let png = std::fs::read(&png_path).with_context(|| format!("failed to read {png_path}"))?;

    section("FILE");
    println!("  path: {png_path}");
    println!("  bytes: {}", png.len());
    if let Some((w, h, bit_depth, color_type)) = png_ihdr(&png) {
        println!("  png: {w}x{h} bit_depth={bit_depth} color_type={color_type}");
    } else {
        println!("  png: <could not read IHDR>");
    }

    match record_decode::infer_record_profile_from_png(&png) {
        Ok(profile) => println!("  inferred profile: {profile}"),
        Err(err) => println!("  inferred profile: <error: {err:#}>"),
    }

    // ---- Decode descriptor + chunk stream from the spiral RGB ---------------
    section("DECODE (RGB spiral -> descriptor + chunk stream)");
    let decoded = record_decode::decode_record_png(&png)
        .context("record_decode::decode_record_png failed")?;
    println!("  decoded profile: {}", decoded.record_profile);

    section("DESCRIPTOR HEADER");
    println!("{}", indent(&format!("{:#?}", decoded.descriptor), 2));
    let d = &decoded.descriptor;
    println!("  -- key fields --");
    println!("  b_value:              {}", d.b_value);
    println!("  stream_byte_length:   {:?}", d.stream_byte_length);
    println!("  record_profile:       {:?}", d.record_profile);
    println!("  payload_encoding:     {:?}", d.payload_encoding);
    println!("  title:                {:?}", d.title);

    let stream = &decoded.chunk_stream.bytes;

    section("CHUNK STREAM (BCS2)");
    println!("  groove pixels:        {}", decoded.chunk_stream.pixel_count);
    println!("  stream bytes:         {}", stream.len());
    println!("  stream magic:         {:?}", ascii_magic(stream));
    if let Some(expected) = d.stream_byte_length {
        let ok = expected == stream.len();
        println!(
            "  descriptor.stream_byte_length == decoded len? {ok}  ({expected} vs {})",
            stream.len()
        );
    }
    println!("  stream prefix:");
    print!("{}", indent(&hex_prefix(stream, 256), 4));

    match record_core::stream_metadata(stream) {
        Ok(meta) => {
            println!("  stream metadata:");
            println!(
                "{}",
                indent(&serde_json::to_string_pretty(&meta).unwrap_or_default(), 4)
            );
        }
        Err(err) => println!("  stream metadata: <error: {err:#}>"),
    }

    // ---- Walk the chunk stream into the payload -----------------------------
    let payload = match record_core::parse_chunk_stream(stream) {
        Ok(parsed) => {
            println!("  chunk count:          {}", parsed.chunks.len());
            for (i, chunk) in parsed.chunks.iter().enumerate().take(8) {
                println!(
                    "    chunk[{i}]: payload_len={} descriptor_index={}",
                    chunk.payload.len(),
                    chunk.payload_descriptor_index
                );
            }
            if parsed.chunks.len() > 8 {
                println!("    ... {} more chunks", parsed.chunks.len() - 8);
            }
            let payload = record_core::chunk_stream_payload_bytes(&parsed);
            println!("  reassembled payload:  {} bytes", payload.len());
            payload
        }
        Err(err) => {
            println!("  parse_chunk_stream FAILED: {err:#}");
            return Ok(());
        }
    };

    section("PAYLOAD");
    println!("  bytes:  {}", payload.len());
    println!("  magic:  {:?}", ascii_magic(&payload));
    println!("  prefix:");
    print!("{}", indent(&hex_prefix(&payload, 128), 4));

    // ---- BPLP length-prefixed entry table -----------------------------------
    let entries: Vec<Vec<u8>> = if record_core::is_payload_entry_length_prefixed_stream(&payload) {
        section("BPLP ENTRY TABLE (length-prefixed)");
        match record_core::payload_entry_length_prefixed_ranges(&payload) {
            Ok(ranges) => {
                println!("  entry count: {}", ranges.len());
                for r in &ranges {
                    let slice = &payload[r.offset..r.offset + r.byte_length];
                    println!(
                        "    entry[{}]: offset={} byte_length={} magic={:?}",
                        r.index,
                        r.offset,
                        r.byte_length,
                        ascii_magic(slice)
                    );
                }
                record_core::parse_payload_entry_length_prefixed_stream(&payload)
                    .context("parse_payload_entry_length_prefixed_stream failed")?
            }
            Err(err) => {
                println!("  payload_entry_length_prefixed_ranges FAILED: {err:#}");
                return Ok(());
            }
        }
    } else {
        section("PAYLOAD IS A SINGLE ENTRY (no BPLP magic)");
        println!("  treating whole payload as one ECDC entry");
        vec![payload.clone()]
    };

    // ---- Optional bundle metadata (to reproduce the exact implied count) ----
    let bundle_meta = match &bundle_path {
        Some(path) => match load_bundle_metadata(path) {
            Ok(meta) => {
                section("BUNDLE METADATA");
                println!(
                    "  {path}: model={} sample_rate={} segment_samples={} segment_stride={}",
                    meta.model_name, meta.sample_rate, meta.segment_samples, meta.segment_stride
                );
                Some(meta)
            }
            Err(err) => {
                eprintln!("  bundle load failed ({path}): {err:#}");
                None
            }
        },
        None => None,
    };

    // ---- Analyse each entry as ECDC -----------------------------------------
    for (i, entry) in entries.iter().enumerate() {
        section(&format!("ECDC ENTRY {i} / {}", entries.len()));
        analyse_ecdc_entry(entry, bundle_meta.as_ref());
    }

    Ok(())
}

fn analyse_ecdc_entry(entry: &[u8], bundle_meta: Option<&encodec_rs::metadata::OnnxFrameBundleMetadata>) {
    println!("  bytes:  {}", entry.len());
    println!("  magic:  {:?}", ascii_magic(entry));
    println!("  prefix:");
    print!("{}", indent(&hex_prefix(entry, 96), 4));

    let mut reader = Cursor::new(entry);
    let metadata: EcdcMetadata = match read_ecdc_header(&mut reader) {
        Ok(meta) => meta,
        Err(err) => {
            println!("  !! read_ecdc_header FAILED: {err:#}");
            return;
        }
    };
    let header_end = reader.position() as usize;
    println!("  ECDC header parsed ok ({header_end} bytes), {} bytes remain for chunks", entry.len() - header_end);
    println!("  -- metadata --");
    println!("    model_name (m):     {}", metadata.model_name);
    println!("    audio_length (al):  {}", metadata.audio_length);
    println!("    num_codebooks (nc): {}", metadata.num_codebooks);
    println!("    use_lm (lm):        {}", metadata.use_lm);
    println!("    bitstream acv:      {}", metadata.bitstream_version);
    println!("    lm_hash (lmh):      {:?}", metadata.lm_hash);
    println!("    chunk_samples (cs): {:?}", metadata.chunk_samples);
    println!("    chunk_stride (cst): {:?}", metadata.chunk_stride);
    println!("    lm_frame_len (fl):  {:?}", metadata.lm_frame_length);
    if !metadata.extra.is_empty() {
        println!("    extra:              {:?}", metadata.extra);
    }

    // Count the actual chunk bodies present after the header.
    let mut raw_chunks = 0usize;
    let mut chunk_err: Option<String> = None;
    loop {
        if (reader.position() as usize) >= entry.len() {
            break;
        }
        match read_chunk_payload(&mut reader, true) {
            Ok(payload) => {
                if raw_chunks < 8 {
                    println!("    chunk[{raw_chunks}]: {} bytes", payload.len());
                }
                raw_chunks += 1;
            }
            Err(err) => {
                chunk_err = Some(format!("{err:#}"));
                break;
            }
        }
    }
    if raw_chunks > 8 {
        println!("    ... {} more chunks", raw_chunks - 8);
    }
    println!("  raw chunk bodies present: {raw_chunks}");
    if let Some(err) = chunk_err {
        println!("  !! chunk read stopped early: {err}");
    }

    // Implied chunk count from audio_length / stride.
    report_implied_chunks(&metadata, bundle_meta, raw_chunks);
}

fn report_implied_chunks(
    metadata: &EcdcMetadata,
    bundle_meta: Option<&encodec_rs::metadata::OnnxFrameBundleMetadata>,
    raw_chunks: usize,
) {
    println!("  -- implied chunk count (audio_length / stride) --");
    if let Some(stride) = metadata.chunk_stride {
        let implied = segment_starts(metadata.audio_length, stride).len();
        println!("    via metadata.chunk_stride={stride}: implies {implied} chunks");
        verdict(raw_chunks, implied);
        return;
    }
    if let Some(meta) = bundle_meta {
        match encodec_rs::format::ecdc_chunk_layout_for_chunk_count(meta, metadata, raw_chunks) {
            Ok(layout) => {
                let implied = segment_starts(metadata.audio_length, layout.stride).len();
                println!(
                    "    via bundle (stride={}): implies {implied} chunks",
                    layout.stride
                );
                verdict(raw_chunks, implied);
            }
            Err(err) => println!("    ecdc_chunk_layout_for_chunk_count: {err:#}"),
        }
        return;
    }
    println!("    (no chunk_stride in metadata and no bundle.json provided)");
    for stride in [metadata.audio_length, 86_400usize, 63_998, 48_000, 32_000] {
        if stride == 0 {
            continue;
        }
        let implied = segment_starts(metadata.audio_length, stride).len();
        println!("    if stride={stride}: implies {implied} chunks");
    }
    println!("    pass a bundle.json as the 2nd arg to compute the exact implied count");
}

fn verdict(raw_chunks: usize, implied: usize) {
    if raw_chunks == implied {
        println!("    => OK: {raw_chunks} chunk bodies match {implied} implied");
    } else {
        println!(
            "    => MISMATCH: {raw_chunks} chunk bodies present but metadata implies {implied} (this is the decode error)"
        );
    }
}

fn load_bundle_metadata(path: &str) -> Result<encodec_rs::metadata::OnnxFrameBundleMetadata> {
    let json = std::fs::read_to_string(Path::new(path))
        .with_context(|| format!("failed to read bundle json {path}"))?;
    serde_json::from_str(&json).context("failed to deserialize OnnxFrameBundleMetadata")
}

// ---- formatting helpers ----------------------------------------------------

fn section(title: &str) {
    println!("\n=== {title} ===");
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn ascii_magic(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|b| if (0x20..=0x7e).contains(b) { *b as char } else { '.' })
        .collect()
}

fn hex_prefix(bytes: &[u8], max: usize) -> String {
    let mut out = String::new();
    let clipped = bytes.len().min(max);
    for offset in (0..clipped).step_by(16) {
        let end = (offset + 16).min(clipped);
        let chunk = &bytes[offset..end];
        let hex = chunk
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii = chunk
            .iter()
            .map(|b| if (0x20..=0x7e).contains(b) { *b as char } else { '.' })
            .collect::<String>();
        out.push_str(&format!("{offset:08x}: {hex:<47}  {ascii}\n"));
    }
    if bytes.len() > max {
        out.push_str(&format!("... truncated {} bytes\n", bytes.len() - max));
    }
    out
}

fn png_ihdr(png: &[u8]) -> Option<(u32, u32, u8, u8)> {
    if png.len() < 33 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((w, h, png[24], png[25]))
}
