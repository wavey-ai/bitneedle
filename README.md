# bitneedle

Bitneedle is the public reference code for reading and validating Bitneedle
picture-record objects: visual record-like images that carry recoverable audio
data.

This repository is published so artists, collectors, archivists, marketplaces,
and independent developers can inspect Bitneedle objects, verify that the audio
is carried by the object itself, and build self-sufficient playback and
preservation tools.

## Licensing

This repository contains components under different licences.

- Decoding, verification, and interoperability crates are licensed under the
  Apache License, Version 2.0, in `LICENSE`.
- Record-authoring crates (the ones that construct, encode, or render
  Bitneedle records) are licensed under the Wavey Artist Source Licence, in
  `LICENSE-ARTIST`. That licence permits qualifying Artists and Artist
  Entities to create and sell their own Bitneedle records, but reserves
  commercial authoring, platform, and label use.

See each crate's `Cargo.toml` and `LICENSE` file for its applicable licence.
Any patent licence is limited to the express terms of the applicable
component licence; publication of this repository is not intended to
dedicate any patent-pending technology to the public or waive patent rights
beyond those express terms. See `PATENTS.md` for the patent notice and
limited decoder pledge.

## Repository scope

| Crate | Licence | Role |
| --- | --- | --- |
| `record-core` | Apache-2.0 | Shared geometry, record-profile, chunk/gap, and spiral-index primitives used by both decoder and authoring tools. |
| `record-descriptor` | Apache-2.0 | BRD1 descriptor wire format, parsing, and decoding. |
| `record-decode` | Apache-2.0 | Decode and inspect Bitneedle picture-record images. |
| `record-verify` | Apache-2.0 | Canonical hashing, registration receipt chains, and signature verification. |
| `record-sidecar` | Apache-2.0 | Sidecar structures used to support recovery and inspection. |
| `record-wasm` | Apache-2.0 | WebAssembly facade for decoding, verification, and sidecar inspection. |
| `player-wasm` | Apache-2.0 | WebAssembly playback/decoding orchestration helpers for Bitneedle player apps (metadata resolution, cache keys, scratch control — no record authoring). |
| `bytes2rgb` | Apache-2.0 | Low-level pixel-to-byte utilities used by decoder and verification tools. |
| `bitneedle-id` | CC0-1.0 | Typed prefixed ULID identifiers for public record objects. |
| `record-groove` | Wavey Artist Source Licence | Byte-to-pixel carrier encoding, toned palette construction, and OKLCH helpers. |
| `record-label` | Wavey Artist Source Licence | Canonical label geometry and spindle/dink cutout authoring primitives. |
| `record-render` | Wavey Artist Source Licence | Constructs and renders the finished record PNG. |
| `record-cut` | Wavey Artist Source Licence | Canonical BRS1 record-stream authoring/encoding, BRD1 descriptor authoring (`descriptor` module), and GAP1 authoring (`gap` module). |
| `record-cut-wasm` | Wavey Artist Source Licence | WebAssembly facade for record authoring: rendering, programme assembly, and record-label profile helpers. |

This repository is not the commercial Bitneedle record authoring platform.
Only Artists and Artist Entities may use the Wavey Artist Source Licence
crates under the terms of that licence; record labels, platforms, and other
commercial users need a separate licence (see `LICENSE-ARTIST` §15).
