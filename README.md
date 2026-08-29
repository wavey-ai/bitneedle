# Bitneedle

Bitneedle is the public reference implementation for Bitneedle picture records.
A Bitneedle picture record is an image that contains recoverable audio data.
Use this code to read and validate a Bitneedle picture record.

Artists, collectors, archivists, marketplaces, and developers can examine a
Bitneedle picture record. They can make sure that the record contains its audio
data. They can also build independent tools to play and preserve the record.

## What a record attests

A pressed record is immutable, and its signature says so: the release
commitment covers the audio and everything the record says about itself —
title, artist, label, catalogue number, copyright, credit, the palette and
the geometry it was cut at. A signed record cannot be re-titled or
re-attributed without becoming a different release.

Three things are allowed to arrive later, and none of them may arrive
unsigned:

- **A chain anchor, an ISRC, a barcode.** None can exist at press time. They
  sit outside the release commitment, and writing any of them requires the
  deferred attestation that signs them, bound to the record they belong to.
- **The sidecar.** It is meant to be rewritten — editions are issued, labels
  are re-authored — so it carries its own attestation, replaced each time it
  is written. Whoever writes it signs what they wrote.

A release may be signed by the artist, by a platform, or by both: they are
separate signatures over one commitment. Who a key belongs to is resolved by
`record-verify` and its caller, never by the wire format.

`bitneedle-format/README.md` sets this out in full.

## Licensing

Different licenses apply to the components in this repository.

- The Apache License, Version 2.0, applies to the decoding, verification, and
  interoperability crates. Refer to `LICENSE`.
- The Wavey Artist Source Licence applies to the record-authoring crates. Refer
  to `LICENSE-ARTIST`.
- Qualifying Artists and Artist Entities can use the record-authoring crates to
  make and sell their own Bitneedle records.
- A label, platform, or other commercial user must get a separate license.

The `Cargo.toml` and `LICENSE` files identify the license for each crate. Only
the applicable component license can grant a patent license. Publication of
this repository does not grant other patent rights. Publication does not put
patent-pending technology in the public domain. Refer to `PATENTS.md` for the
patent notice and the limited decoder pledge.

## Repository scope

| Crate | License | Role |
| --- | --- | --- |
| `record-core` | Apache-2.0 | Shared geometry, record-profile, chunk/gap, and spiral-index primitives used by both decoder and authoring tools. |
| `record-descriptor` | Apache-2.0 | BRD1 descriptor wire format, parsing, and decoding, including the signed identity a release commitment is taken over. |
| `record-package` | Apache-2.0 | Optional BPK1 container for exact BRD1, BRS1, and BSC1 component bytes. |
| `record-decode` | Apache-2.0 | Decode and inspect Bitneedle picture-record images. |
| `record-verify` | Apache-2.0 | Canonical hashing, registration receipt chains, and signature verification. |
| `record-sidecar` | Apache-2.0 | Sidecar structures used to support recovery and inspection, and the attestation a sidecar carries over its own contents. |
| `record-wasm` | Apache-2.0 | WebAssembly facade for decoding, verification, and sidecar inspection. |
| `player-wasm` | Apache-2.0 | WebAssembly playback/decoding orchestration helpers for Bitneedle player apps (metadata resolution, cache keys, scratch control — no record authoring). |
| `bytes2rgb` | Apache-2.0 | Low-level pixel-to-byte utilities used by decoder and verification tools. |
| `bitneedle-id` | CC0-1.0 | Typed prefixed ULID identifiers for public record objects. |
| `record-groove` | Wavey Artist Source Licence | Byte-to-pixel carrier encoding, toned palette construction, and OKLCH helpers. |
| `record-label` | Wavey Artist Source Licence | Canonical label geometry and spindle/dink cutout authoring primitives. |
| `record-render` | Wavey Artist Source Licence | Constructs and renders the finished record PNG. |
| `record-cut` | Wavey Artist Source Licence | Canonical BRS1 record-stream authoring/encoding, BRD1 descriptor authoring (`descriptor` module), and GAP1 authoring (`gap` module). |
| `record-cut-wasm` | Wavey Artist Source Licence | WebAssembly facade for record rendering, program assembly, and record-label profile helpers. |

This repository is not the commercial Bitneedle record authoring platform.
Only Artists and Artist Entities can use the applicable crates under the Wavey
Artist Source Licence. Record labels, platforms, and other commercial users
need a separate license. Refer to section 15 of `LICENSE-ARTIST`.
