# bytes2rgb

Decoder-side color utilities for Bitneedle.

This crate recovers byte streams from RGBA pixel data used by Bitneedle
picture-record objects. It intentionally documents and exposes the read path:
RGB byte recovery, grayscale metadata recovery, exact toned-palette recovery,
and PNG decoding for inspection tools.

It is not a Bitneedle record authoring crate and does not grant rights to
create, mint, issue, sell, or market Bitneedle records.
