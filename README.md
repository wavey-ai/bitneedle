# Bitneedle

Bitneedle is the public reference implementation for Bitneedle picture records.
A Bitneedle picture record is an image that contains recoverable audio data.

## Record geometry

Every profile takes its dimensions from the RIAA standard. The three sizes
differ in their outer diameter and in where the recorded band starts; a 10 in
and a 12 in share an inner recording diameter, and all three share a centre
hole.

| | 7 in `single45` | 10 in `ten` | 12 in `lp` |
| --- | --- | --- | --- |
| Outside diameter | 6 7/8 in = **174.6 mm** | 9 7/8 in = **250.8 mm** | 11 7/8 in = **301.6 mm** |
| Outermost groove at recording pitch | 6 5/8 in = **168.3 mm** | 9 1/2 in = **241.3 mm** | 11 1/2 in = **292.1 mm** |
| Minimum inside diameter of recording | 4 1/4 in = **107.95 mm** | 4 3/4 in = **120.65 mm** | 4 3/4 in = **120.65 mm** |
| Label diameter | 92.1 mm | 100.0 mm | 100.0 mm |
| Centre hole | 7.5 mm (+ 38.1 mm dink) | 7.24 mm | 7.24 mm |

Label diameter is the one dimension the RIAA standard leaves to the plant, so
these three come from the curated pressing-plant template registry in
`record-plant`, where the published trim clusters hard: 92 mm on a 7 in (25 of
44 templates), 100 mm on a 12 in (18 of 26), and 100 mm on a 10 in (6 of 9).
Most plants ship one shared "10/12 in label" template, and the plants that draw
a dedicated 10 in file still specify 100 mm. A 10 in may also be pressed with a
7 in label, which plants offer as a customer option; 100 mm stays the default.

`margin_diameter_mm` is the format's own value. It sets the outer margin band,
and each profile places it proportionally within the gap between the outermost
groove and the disc edge.

### The groove, rim to label

One groove is cut, and it runs the whole way. The stylus travels left to
right through every band below without a discontinuity — the deadwax picks
the groove up on the exact angle the programme put it down, and a reader
walks straight out of the payload into it.

```
   travel of the stylus  ──────────────────────────────────────────────▶

   r_outer      payload_outer     cut_inner    payload_inner    label_radius
      │              │                 │             │               │
      ▼              ▼                 ▼             ▼               ▼
   ┌──────┬──────────────┬─────────────────┬─────────────────┬────────────┐
   │ rim  │   LEAD-IN    │    PROGRAMME    │     DEADWAX     │  RUN-OUT   │
   │      │              │                 │                 │            │
   │ flat │  2 turns     │  pitch = b      │  1.00 mm/turn   │  4 turns   │
   │ no   │  fixed       │  from the fit   │  true physical  │  fixed     │
   │groove│              │                 │                 │            │
   ├──────┼──────────────┼─────────────────┼─────────────────┼────────────┤
   │  ―   │ BRD1  [1/2]  │  audio payload  │  free carrier   │ BRD1 [2/2] │
   │      │ grey nibble  │  rgb / toned    │  unwritten      │grey nibble │
   │      │ 4 bits/px    │  20 bits/px     │  today          │ 4 bits/px  │
   └──────┴──────────────┴─────────────────┴─────────────────┴────────────┘
                          └── the cut stops wherever the programme ran out;
                              everything it did not reach is deadwax
```

**Lead-in** — two fixed turns at the rim. Carries the first half of the BRD1
descriptor, including the 29-byte prefix a decoder reads before anything
else. Because the prefix must be readable before any palette is known, this
band is painted in grey nibbles (base 120, one nibble per pixel).

**Programme** — the audio, at the pitch the fit solved for. It is laid out
from the rim against a nominal span and stops at `cut_inner_radius`, which
the prefix carries.

**Deadwax** — from where the programme stopped, in to the run-out. Cut at a
true physical 1 mm per turn, so it is the one band rendered at life size,
and its turn count is whatever travel the programme left over — typically 40
to 90 turns on a short cut. It is a carrier in its own right: an ordered,
addressable pixel sequence reproducible from the prefix alone. Declared by
`SEGMENT_DEADWAX_EXTENT` (33) and offered to sidecars, which must write it
as a groove, not as a canvas to paint on. A programme that fills the band
leaves no deadwax and writes no segment at all.

**Run-out** — the four fixed turns around the label. Reserved for strict
header metadata: it carries the second half of the BRD1 stream and is never
offered to a sidecar.

The exact radii, in rendered pixels on the 576 x 576 canvas:

| | `single45` | `ten` | `lp` |
| --- | --- | --- | --- |
| Disc edge | 287 | 287 | 287 |
| Outer rim thickness | 4 | 4 | 4 |
| Lead-in band thickness | 6 | 7 | 5 |
| Payload outer radius | 280 | 279 | 281 |
| Payload inner radius | 177 | 138 | 115 |
| Label radius | 151 | 114 | 95 |
| Spindle hole radius | 12 | 8 | 7 |
| Lead-in turns | 2 | 2 | 2 |
| Run-out turns | 4 | 4 | 4 |
| Deadwax pitch, px/turn | 3.2875 | 2.2884 | 1.9032 |
| Pixels per mm | 3.2875 | 2.2884 | 1.9032 |

The deadwax pitch equals the pixels-per-mm figure exactly, because the band
is cut at 1.00 mm per turn by definition. Every other pitch in the table is
derived from a turn count and the travel available, not chosen.

There is no lock groove. A pressed record ends its run-out in a closed
concentric circle that traps the stylus; Bitneedle's run-out currently stops
at the label radius instead. That terminator is not yet cut.

### How this reaches the 576 x 576 canvas

The rendered disc always fills the canvas — `outer_radius_px` is **287** for
every profile — so every format draws at one size whatever its physical
diameter. Format identity on screen is carried entirely by **the
label-to-disc ratio**, because a near-constant label on a shrinking disc
occupies more of it:

| Profile | Label radius | Ratio of canvas | Label / disc | Payload band |
| --- | --- | --- | --- | --- |
| `single45` | 151 px | 0.5243 | **52.7 %** | 169 -> 280 px (111 px) |
| `ten` | 114 px | 0.3958 | **39.9 %** | 130 -> 280 px (150 px) |
| `lp` | 95 px | 0.3299 | **33.2 %** | 109 -> 280 px (171 px) |

Radii are scaled from each profile's own physical geometry and rounded to whole
pixels:

```
scale        = outer_radius_px / (finished_diameter_mm / 2)
feature_px   = round((feature_mm / 2) * scale)
```

Reversing that rounding recovers the physical label to within 0.4 mm — 91.9 mm,
99.6 mm, and 99.8 mm against plant targets of 92, 100, and 100 — so the drawn
proportions are accurate to the real object, and a UI may take the ratio above
as the truthful glyph for a format.

`margin_radius` lands on 283 px for all three profiles, so every format shares
an `outer_rim_thickness` of 4 px, a `lead_in_band_thickness` of 6 px, and a
`payload_outer_radius` of 280 px. The inner edge of the payload band is the one
edge that moves, and it moves with the label.

## What a record attests

A pressed record is immutable, and its signature says so: the release
commitment covers the audio and everything the record says about itself —
title, artist, label, catalogue number, copyright, credit, the palette and
the geometry it was cut at. Re-titling or re-attributing a signed record
produces a different release.

Three things are allowed to arrive later, and each of them arrives signed:

- **A chain anchor, an ISRC, a barcode.** Each is issued after the press: an
  anchor needs a commitment to anchor, and registrars assign ISRCs and barcodes
  on their own schedule. They sit outside the release commitment, and writing
  any of them requires the deferred attestation that signs them, bound to the
  record they belong to.
- **The sidecar.** It is meant to be rewritten — editions are issued, labels
  are re-authored — so it carries its own attestation, replaced each time it
  is written. Whoever writes it signs what they wrote.

A release may be signed by the artist, by a platform, or by both: they are
separate signatures over one commitment. Who a key belongs to is resolved by
`record-verify` and its caller, outside the wire format.

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

The `Cargo.toml` and `LICENSE` files identify the license for each crate. A
patent license comes from the applicable component license and from that alone.
Every other patent right stays reserved on publication, and patent-pending
technology stays patent pending. Refer to `PATENTS.md` for the patent notice
and the limited decoder pledge.

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
| `player-wasm` | Apache-2.0 | WebAssembly playback/decoding orchestration helpers for Bitneedle player apps (metadata resolution, cache keys, scratch control; the playback side alone). |
| `bytes2rgb` | Apache-2.0 | Low-level pixel-to-byte utilities used by decoder and verification tools. |
| `bitneedle-id` | CC0-1.0 | Typed prefixed ULID identifiers for public record objects. |
| `record-groove` | Wavey Artist Source Licence | Byte-to-pixel carrier encoding, toned palette construction, and OKLCH helpers. |
| `record-label` | Wavey Artist Source Licence | Canonical label geometry and spindle/dink cutout authoring primitives. |
| `record-render` | Wavey Artist Source Licence | Constructs and renders the finished record PNG. |
| `record-cut` | Wavey Artist Source Licence | Canonical BRS1 record-stream authoring/encoding, BRD1 descriptor authoring (`descriptor` module), and GAP1 authoring (`gap` module). |
| `record-cut-wasm` | Wavey Artist Source Licence | WebAssembly facade for record rendering, program assembly, and record-label profile helpers. |

This repository is the public reference implementation; the commercial
Bitneedle record authoring platform is separate. Use of the applicable crates
under the Wavey Artist Source Licence is limited to Artists and Artist
Entities. Record labels, platforms, and other commercial users need a separate
license. Refer to section 15 of `LICENSE-ARTIST`.
