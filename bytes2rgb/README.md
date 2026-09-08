# bytes2rgb

Decoder-side color utilities for Bitneedle.

This crate recovers byte streams from the RGBA pixel data of Bitneedle
picture-record objects. It documents and exposes four read operations: RGB byte
recovery, grayscale metadata recovery, exact toned-palette recovery, and PNG
decoding for inspection tools.

It is not a Bitneedle record authoring crate and does not grant rights to
create, mint, issue, sell, or market Bitneedle records.
