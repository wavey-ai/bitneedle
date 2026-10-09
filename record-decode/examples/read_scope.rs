use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::time::Instant;

fn timed<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let start = Instant::now();
    let value = action()?;
    println!("{name}: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
    Ok(value)
}

fn main() -> Result<()> {
    let file = std::env::args()
        .nth(1)
        .context("Pass a record image path")?;
    let png = std::fs::read(&file)?;
    println!("file: {file}; bytes: {}", png.len());
    for run in 1..=3 {
        println!("run: {run}");
        let (width, height, _) = timed("PNG pixels", || record_decode::load_record_rgba(&png))?;
        let (profile, descriptor) = timed("descriptor with profile discovery", || {
            record_decode::decode_record_descriptor_from_png(&png, None)
        })?;
        timed("descriptor with known profile", || {
            record_decode::decode_record_descriptor_from_png(&png, Some(&profile))
        })?;
        let decoded = timed("full groove recovery", || {
            record_decode::decode_record_png(&png)
        })?;
        let stream = timed("BRS1 parse", || {
            record_core::parse_record_stream(&decoded.chunk_stream.bytes)
        })?;
        let map = timed("programme map", || {
            record_core::build_programme_map(&stream, Some(&profile))
        })?;
        timed("audio fingerprints", || {
            for region in &map.regions {
                if let record_core::ProgrammeRegionKind::Track { number, .. } = &region.kind {
                    let track = &stream.metadata.tracks[number - 1];
                    let mut hash = Sha256::new();
                    hash.update(map.sample_rate.to_le_bytes());
                    hash.update(region.sample_count.to_le_bytes());
                    for chunk in &stream.chunks[track.first_revolution_index
                        ..track.first_revolution_index + track.revolution_count]
                    {
                        hash.update(&chunk.payload);
                    }
                    std::hint::black_box(hash.finalize());
                }
            }
            Ok(())
        })?;
        println!(
            "{width}x{height}; {profile}; {}; stream: {} bytes; tracks: {}; duration: {:.3} s",
            descriptor.payload_encoding,
            decoded.chunk_stream.bytes.len(),
            stream.metadata.tracks.len(),
            map.total_samples as f64 / f64::from(map.sample_rate)
        );
    }
    Ok(())
}
