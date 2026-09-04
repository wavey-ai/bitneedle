fn main() -> anyhow::Result<()> {
    let png = std::fs::read(std::env::args().nth(1).unwrap())?;
    let decoded = record_decode::decode_record_png(&png)?;
    println!("profile:        {}", decoded.record_profile);
    println!("groove pixels:  {}", decoded.chunk_stream.pixel_count);
    println!("stream bytes:   {}", decoded.chunk_stream.bytes.len());
    let head = &decoded.chunk_stream.bytes[..4.min(decoded.chunk_stream.bytes.len())];
    println!("stream magic:   {:?}", std::str::from_utf8(head));
    Ok(())
}
