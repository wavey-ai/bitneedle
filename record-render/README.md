# record-render

This crate holds the reference Bitneedle rendering helpers. Compatible
authoring tools and preview tools use them. The BRD1 wire format is defined in
`record-descriptor`.

## Export formats

A record is data. Each groove pixel holds part of the payload. An RGB cut
holds three bytes in each pixel. A toned cut holds one of 1,048,576 colours
in each pixel, at 20 bits per pixel.

A format must return each channel of each pixel exactly. A lossy format
changes pixels. A palette format holds a maximum of 256 colours. Both
formats prevent the decode.

The decoder finds the end of the groove at the first pixel with an alpha
value of 0. Thus a format must also keep the alpha channel.

`export::write_rgba` writes a record in these formats:

| Format | Feature | Notes |
| ------ | ------- | ----- |
| PNG    | always  | What the press writes. |
| TIFF   | `tiff`  | Uncompressed. |
| BMP    | `bmp`   | Uncompressed. `BITMAPV4HEADER`, which keeps the alpha channel. |
| QOI    | `qoi`   | The fastest encode. |
| WebP   | `webp`  | VP8L. This encoder writes lossless WebP only. |
| TGA    | `tga`   | Run-length encoded. |
| PAM    | `pnm`   | The netpbm arbitrary map, `P7`. PPM has no alpha channel. |
| Raw    | `rgba`  | A magic value, the width, the height, then the pixels. |

Each feature also turns on the format in `record-decode`. Thus a build can
read each format that it can write.

`export::read_rgba` reads a record back. It identifies the format from the
bytes. TGA has no magic value, thus the reader tries TGA last.

`all-formats` turns on all of the formats.

To measure the formats, run this command:

```sh
cargo run --release -p record-render --features all-formats \
  --example export_formats
```

## License

This crate is source-available under the Wavey Artist Source Licence.
Individual artists and artist-controlled entities may use it free of charge to
create and sell records containing their own work.
Record labels, platforms, hosted services, technology providers, and other
commercial users require a separate license from Wavey, Inc.
Commercial licensing: license@yl.vin
This crate is not open-source software.
