use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("record-decode should live one level below repository root")
        .to_path_buf()
}

fn debug_png_path() -> PathBuf {
    repo_root()
        .join("goldenfiles")
        .join("debug")
        .join("failing-record.png")
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

fn hex_prefix(bytes: &[u8], max: usize) -> String {
    let mut out = String::new();
    let clipped = bytes.len().min(max);

    for offset in (0..clipped).step_by(16) {
        let end = (offset + 16).min(clipped);
        let chunk = &bytes[offset..end];

        let hex = chunk
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");

        let ascii = chunk
            .iter()
            .map(|byte| {
                if (0x20..=0x7e).contains(byte) {
                    *byte as char
                } else {
                    '.'
                }
            })
            .collect::<String>();

        out.push_str(&format!("{offset:08x}: {hex:<47}  {ascii}\n"));
    }

    if bytes.len() > max {
        out.push_str(&format!("... truncated {} bytes\n", bytes.len() - max));
    }

    out
}

fn ascii_magic(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|byte| {
            if (0x20..=0x7e).contains(byte) {
                *byte as char
            } else {
                '.'
            }
        })
        .collect()
}

fn find_magic(bytes: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || bytes.len() < needle.len() {
        return Vec::new();
    }

    bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect()
}

#[test]
#[ignore = "debug fixture requires manual PNG inspection"]
fn debug_png_decodes_chunk_stream_header_by_header() -> Result<()> {
    let path = debug_png_path();
    let png = read_file(&path)?;

    println!("file: {}", path.display());
    println!("png bytes: {}", png.len());

    let decoded = record_decode::decode_record_png(&png)
        .context("record_decode::decode_record_png failed")?;

    println!("profile: {}", decoded.record_profile);
    println!("descriptor:");
    println!("{:#?}", decoded.descriptor);

    let descriptor = &decoded.descriptor;

    println!("descriptor checks:");
    println!("  b_value: {}", descriptor.b_value);
    println!("  stream_byte_length: {:?}", descriptor.stream_byte_length);
    println!("  record_profile: {:?}", descriptor.record_profile);
    println!("  generation_version: {:?}", descriptor.generation_version);
    println!("  payload_encoding: {:?}", descriptor.payload_encoding);
    println!("  title: {:?}", descriptor.title);
    println!("  artist: {:?}", descriptor.artist);
    println!("  release_id: {:?}", descriptor.release_id);
    println!("  canonical_url: {:?}", descriptor.canonical_url);
    println!("  created_at: {:?}", descriptor.created_at);
    println!(
        "  signature_algorithm: {:?}",
        descriptor.signature_algorithm
    );
    println!("  signature_key_id: {:?}", descriptor.signature_key_id);
    println!("  manifest_sha256: {:?}", descriptor.manifest_sha256);
    println!(
        "  signed_release_manifest bytes: {}",
        descriptor
            .signed_release_manifest
            .as_deref()
            .map(str::len)
            .unwrap_or(0)
    );
    println!(
        "  signature bytes: {}",
        descriptor.signature.as_deref().map(str::len).unwrap_or(0)
    );

    if let Some(expected_len) = descriptor.stream_byte_length {
        if expected_len != decoded.chunk_stream.bytes.len() {
            bail!(
                "descriptor stream_byte_length mismatch: descriptor says {}, decoded stream has {}",
                expected_len,
                decoded.chunk_stream.bytes.len()
            );
        }
    }

    let stream = &decoded.chunk_stream.bytes;

    println!(
        "decoded groove pixels: {}",
        decoded.chunk_stream.pixel_count
    );
    println!("decoded chunk stream bytes: {}", stream.len());
    println!("decoded chunk stream magic: {:?}", ascii_magic(stream));
    println!("decoded chunk stream prefix:");
    println!("{}", hex_prefix(stream, 512));

    println!("magic hits in decoded stream:");
    for magic in [
        b"BCS2".as_slice(),
        b"ECDC".as_slice(),
        b"BRD1".as_slice(),
        b"{".as_slice(),
    ] {
        let hits = find_magic(stream, magic);
        println!(
            "  {:?}: {} hit(s), first {:?}",
            String::from_utf8_lossy(magic),
            hits.len(),
            hits.iter().take(12).collect::<Vec<_>>()
        );
    }

    if stream.len() < record_core::STREAM_HEADER_LENGTH {
        bail!("decoded chunk stream is shorter than BCS2 stream header");
    }

    if &stream[..4] != record_core::CHUNK_STREAM_MAGIC {
        bail!(
            "decoded chunk stream magic mismatch: got {:?}, expected {:?}",
            ascii_magic(stream),
            String::from_utf8_lossy(record_core::CHUNK_STREAM_MAGIC)
        );
    }

    let metadata_end =
        record_core::stream_header_end(stream).context("chunk stream header failed")?;

    let metadata_bytes = record_core::stream_metadata_bytes(stream)
        .context("chunk stream metadata byte extraction failed")?;

    let metadata =
        record_core::stream_metadata(stream).context("chunk stream metadata JSON failed")?;

    println!("stream header:");
    println!("  metadata_end: {metadata_end}");
    println!("  metadata_bytes: {}", metadata_bytes.len());
    println!("stream metadata:");
    println!("{}", serde_json::to_string_pretty(&metadata)?);

    let parsed = record_core::parse_chunk_stream(stream).context("parse_chunk_stream failed")?;

    let ranges = record_core::chunk_all_ranges(stream).context("chunk_all_ranges failed")?;

    if parsed.chunks.len() != ranges.len() {
        bail!(
            "parsed chunk count/range count mismatch: {} vs {}",
            parsed.chunks.len(),
            ranges.len()
        );
    }

    println!("chunks: {}", parsed.chunks.len());

    let mut offset = metadata_end;
    let mut total_payload_bytes = 0usize;

    for expected_index in 0..parsed.chunks.len() {
        let header = record_core::read_chunk_header(stream, offset)
            .with_context(|| format!("failed to read chunk header at offset {offset}"))?;

        let range = &ranges[expected_index];
        let chunk = &parsed.chunks[expected_index];

        let payload = stream
            .get(header.payload_start..header.payload_end)
            .context("chunk payload range outside stream")?;

        let actual_crc32 = record_core::crc32_ieee(payload);

        println!("chunk {expected_index}:");
        println!("  chunk_start: {}", header.chunk_start);
        println!("  chunk_end: {}", header.payload_end);
        println!("  byte_length: {}", header.payload_end - header.chunk_start);
        println!("  index: {}", header.index);
        println!("  chunk_count: {}", header.chunk_count);
        println!("  payload_start: {}", header.payload_start);
        println!("  payload_end: {}", header.payload_end);
        println!("  payload_len: {}", header.payload_len);
        println!("  signature_start: {}", header.chunk_start + 12);
        println!(
            "  signature_end: {}",
            header.chunk_start + 12 + record_core::CHUNK_SIGNATURE_LENGTH
        );
        println!("  crc32 declared: 0x{:08x}", header.crc32);
        println!("  crc32 computed: 0x{:08x}", actual_crc32);
        println!("  range chunk: {}..{}", range.chunk.start, range.chunk.end);
        println!(
            "  range payload: {}..{}",
            range.payload.start, range.payload.end
        );
        println!(
            "  range signature: {}..{}",
            range.signature.start, range.signature.end
        );
        println!("  payload prefix:");
        println!("{}", hex_prefix(payload, 96));

        if header.index as usize != expected_index {
            bail!(
                "chunk index mismatch at offset {}: expected {}, got {}",
                offset,
                expected_index,
                header.index
            );
        }

        if header.chunk_count as usize != parsed.chunks.len() {
            bail!(
                "chunk count mismatch at chunk {}: header says {}, parsed count is {}",
                expected_index,
                header.chunk_count,
                parsed.chunks.len()
            );
        }

        if header.crc32 != actual_crc32 {
            bail!("chunk CRC32 mismatch at chunk {expected_index}");
        }

        if header.payload_len != chunk.payload.len() {
            bail!(
                "parsed payload len mismatch at chunk {}: header says {}, parsed has {}",
                expected_index,
                header.payload_len,
                chunk.payload.len()
            );
        }

        if range.chunk.start != header.chunk_start || range.chunk.end != header.payload_end {
            bail!("chunk_all_ranges chunk range mismatch at chunk {expected_index}");
        }

        if range.payload.start != header.payload_start || range.payload.end != header.payload_end {
            bail!("chunk_all_ranges payload range mismatch at chunk {expected_index}");
        }

        total_payload_bytes += header.payload_len;
        offset = header.payload_end;
    }

    if offset != stream.len() {
        bail!(
            "chunk walk did not consume stream exactly: stopped at {}, stream len {}",
            offset,
            stream.len()
        );
    }

    let reconstructed = record_core::chunk_stream_payload_bytes(&parsed);

    println!("payload bytes from headers: {total_payload_bytes}");
    println!("payload bytes reconstructed: {}", reconstructed.len());
    println!("payload prefix:");
    println!("{}", hex_prefix(&reconstructed, 256));

    if total_payload_bytes != reconstructed.len() {
        bail!(
            "payload total mismatch: headers total {}, reconstructed {}",
            total_payload_bytes,
            reconstructed.len()
        );
    }

    println!("throughout debug PNG check passed");

    Ok(())
}
