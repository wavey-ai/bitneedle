# Bitneedle

Bitneedle is the public reference implementation for Bitneedle picture records.
A Bitneedle picture record is an image that contains recoverable audio data.

## Record geometry

Every profile takes its dimensions from the RIAA standard. The three sizes have
three outer diameters, and each size starts its recorded band at its own
diameter. A 10 in and a 12 in share one inner recording diameter. All three
sizes share one centre hole.

| | 7 in `single45` | 7 in `single45vintage` | 10 in `ten` | 12 in `lp` |
| --- | --- | --- | --- | --- |
| Outside diameter | 6 7/8 in = **174.6 mm** | 6 7/8 in = **174.6 mm** | 9 7/8 in = **250.8 mm** | 11 7/8 in = **301.6 mm** |
| Outermost groove at recording pitch | 6 5/8 in = **168.3 mm** | 6 5/8 in = **168.3 mm** | 9 1/2 in = **241.3 mm** | 11 1/2 in = **292.1 mm** |
| Minimum inside diameter of recording | 4 1/4 in = **107.95 mm** | 4 1/4 in = **107.95 mm** | 4 3/4 in = **120.65 mm** | 4 3/4 in = **120.65 mm** |
| Label diameter | 92.1 mm | **84.0 mm** | 100.0 mm | 100.0 mm |
| Centre hole | 7.5 mm (+ 38.1 mm dink) | 7.5 mm (+ 38.1 mm dink) | 7.24 mm | 7.24 mm |

The RIAA standard leaves the label diameter to the plant. These figures
therefore come from the curated pressing-plant template registry in
`record-plant`. The published trim collects at three values: 92 mm on a 7 in
(25 of 44 templates), 100 mm on a 12 in (18 of 26), and 100 mm on a 10 in
(6 of 9).

The 7 in has a second group of trim values, and `single45vintage` carries it.
Ten templates trim at 83.6–84 mm: GZ, Memphis, Press On, Precision, Sonic Wax UK
and XVINYLX. The other twenty-five templates trim at 92 mm. The hole size and
the trim value are independent. Precision publishes 83.94 mm for its small-hole
labels and for its large-hole labels. Gotta Groove publishes 92.07 mm for both.
Thirteen templates cover either hole under one spec. A record therefore carries
a profile and a hole independently, and a dinked 1950s single is
`single45vintage` with its knockout removed. The smaller paper leaves 39 px of
label clearance, against 26 px on the modern profile. The run-out and the locked
groove are cut in that clearance.

Both 7 in profiles carry a 35.1 mm knockout inside the 38.1 mm dink. The
knockout is the removable centre. Its diameter is the dink less a 1.5 mm
perforation on each side. A plant specifies that perforation. A caliper laid
across the knockout reads the molded edge and gives 35 mm.

Most plants ship one shared "10/12 in label" template. A plant that draws a
dedicated 10 in file also specifies 100 mm. Plants offer a 7 in label on a 10 in
record as a customer option, and 100 mm stays the default.

`margin_diameter_mm` is the format's own value. It sets the outer margin band,
and each profile places it proportionally within the gap between the outermost
groove and the disc edge.

### The groove, rim to label

The cut is one continuous groove from the rim to the label. The stylus travels
left to right through every band below. The silent groove takes the groove up at the
exact angle at which the programme leaves it, so a reader passes from the
payload into the silent groove in one move. The groove ends in a closed ring, which is
the locked groove.

```
   travel of the stylus  ──────────────────────────────────────────────────▶

   r_outer   payload_outer   cut_inner        entry      lock   label_radius
      │           │              │              │          │         │
      ▼           ▼              ▼              ▼          ▼         ▼
   ┌──────┬────────────┬───────────────┬─────────────┬──────────┬─────────┐
   │ rim  │  LEAD-IN   │   PROGRAMME   │   SILENT_GROOVE   │ RUN-OUT  │  LOCK   │
   │      │            │               │             │          │         │
   │ flat │ 2 turns    │ pitch = b     │ 1.00 mm/trn │ fills the│ 1 turn  │
   │ no   │ fixed      │ from the fit  │ true physical│ room     │ closed  │
   │groove│            │               │ ≤ 6 turns   │ 3.2-5 mm │ 3.5 mm  │
   ├──────┼────────────┼───────────────┼─────────────┼──────────┼─────────┤
   │  ―   │ BRD1 [1/2] │ audio payload │ free carrier│ BRD1 [2/2]         │
   │      │ grey 96-159│ rgb / toned   │ the record's│ matte, iso-luma    │
   │      │ 6 bits/px  │ 20 bits/px    │ own tone    │ 6 bits/px          │
   └──────┴────────────┴───────────────┴─────────────┴──────────┴─────────┘
                        └── the cut stops wherever the programme ran out;
                            the run-out claims what it can reach of the rest
```

**Lead-in** — two fixed turns at the rim. This band carries the whole BRD1
descriptor, including the 29-byte prefix that a decoder reads first. A decoder
must read the prefix before it knows any palette, so this band is painted in
plain grey: 64 consecutive rungs, 96 to 159, at six bits to a pixel. The window
sits at the centre of the range. The lead-in and the run-out are visible rings,
and a ladder that reaches black and white draws them as a barcode. Sixty-four
rungs one value apart need sixty-four values, so the band sits in the middle
with 96 values of headroom on each side. A decoder matches each pixel to one
exact rung. The path from cutter to reader is lossless, so a pixel off the
ladder is a corrupted pixel, and the decoder refuses it.

The lead-in is the one grey band. The ladder is the encoding of the bootstrap.
The silent groove and the trailer are grooves on a picture record, and they are cut in
the colours of the picture. The stream fills the lead-in first, to its capacity
of 2 428 bytes on an LP. A toned record with its wheel, title, artist, label,
catalogue number and URL weighs 282 bytes. The stream crosses into the trailer
after the lead-in is full. The split falls on a byte boundary, so each band
packs its own bits.

The ladder changed at draft-05, and that change breaks compatibility. The
ladder encodes the descriptor itself, so a record cut under draft-04 fails at
the BRD1 magic and stays unreadable. A field inside the descriptor cannot
version this change, because the descriptor is the unreadable part. Re-cut a
record of that vintage.

**Programme** — the audio, at the pitch the fit solved for. It is laid out
from the rim against a nominal span and stops at `cut_inner_radius`, which
the prefix carries.

**Silent groove** — from the end of the programme in to the outermost ring of the
run-out. The silent groove is cut at a true physical 1 mm per turn, so it is the one
band rendered at life size. It is a header of at most **6 turns**. The head
feeds at the spiral rate of the lathe, and the extent of the band is unknown to
it, so this limit keeps a short side from filling the annulus with a
one-millimetre ladder. The run-out takes the space below the header.

The silent groove is a carrier: an ordered, addressable pixel sequence, reproducible
from the prefix alone. It is cut in the tone of the record. Each pixel takes the
colour of the wheel pocket it passes through, which is the colour that the
programme above it is cut in. The change of feed is therefore a change of pitch
at one colour. `SEGMENT_SILENT_GROOVE_EXTENT` (33) declares the band, and the format
offers it to sidecars, which write it as a groove. A record that offers no
palette for the band declares no extent.

**Run-out** — widely spaced rings from the lock up to the silent groove header. The
gaps open outward from a fixed 3.2 mm at the inside, each ring standing 1.5
times further out than the one within, until they reach the lathe's own coarse
feed of 5 mm. Every ring above that sits at the feed. Unclamped the taper is
exponential: a sixth ring alone would be 24 mm, and a band long enough to cross
a short side's annulus would be four rings with a bare disc above them.

The ring count is **derived**. The band fills the space between the lock and the
header, and the count is the ladder that comes nearest that space. A 12" cut a
third of the way down carries twelve rings, and a 12" cut to the label carries
one ring. The encoder and the decoder compute the same band from
`cut_inner_radius`, which the prefix carries. The wire holds no count, so the
two sides compute one answer.

The gaps absorb the remainder that a whole number of rings leaves. The band
begins at the ceiling, and the ladder spans the whole descent. Silent groove at the
fine feed fills the space above the band, so a band that stops one gap short
costs five more turns of ladder.

**Locked groove** — one closed revolution 3.5 mm out from the label edge. A
pressed record puts its lock groove at that radius and keeps the annulus inside
it smooth for matrix and stamper marks. Its pixel sequence is cyclic: a reader
that passes the end of the lock returns to the first pixel of the lock. A reader
passes the run-out once and repeats the lock.

The run-out and the lock together are the trailer carrier — one ordered
sequence holding whatever of the BRD1 stream outgrew the lead-in, guaranteed to
hold **512 bytes** on every profile at every extent, and a filled band holds
several times that.

The trailer rings cross the artwork nearest the label. They carry data rather
than a visible message. They are therefore cut in a colour that the picture
already holds at that radius, and they read as part of the record.

The trailer takes one of two tonings. A record with a wheel cuts the trailer by
the wheel: each pixel takes the colour of the pocket it passes through, which is
the colour the band it crosses was read in. A record without a wheel cuts the
trailer in one tone. The caller chooses that tone by reading the artwork, and
`SEGMENT_LEAD_OUT_GEOMETRY` (35) names it on the wire.

Both tonings make the band writable. The bytes use the same 64-symbol ladder at
the same six bits to a pixel, in **iso-luma colours around a base tone**. The
palette varies the colour and holds the luma constant, so a written ring and an
unwritten ring carry the same brightness. The palette derives from the base tone
alone, as the tightest luma window that yields 64 colours. A wheeled trailer
derives one palette for each pocket, from that pocket's own tone.

A reader needs the toning before it reaches the band. Segment 35 is therefore
written at the front of the body, before any field whose length a writer
chooses. A wheeled trailer needs the tone clock map instead, and that map must
fit in the lead-in. The encoder refuses a cut that satisfies neither condition.

The exact radii, in rendered pixels on the 576 x 576 canvas:

| | `single45` | `single45vintage` | `ten` | `lp` |
| --- | --- | --- | --- | --- |
| Disc edge | 287 | 287 | 287 | 287 |
| Outer rim thickness | 4 | 4 | 4 | 4 |
| Lead-in band thickness | 6 | 6 | 7 | 5 |
| Payload outer radius | 280 | 280 | 279 | 281 |
| Payload inner radius | 177 | 177 | 138 | 115 |
| Label radius | 151 | 138 | 114 | 95 |
| Spindle hole radius | 12 | 12 | 8 | 7 |
| Dink radius | 63 | 63 | none | none |
| Dink knockout radius | 58 | 58 | none | none |
| Lead-in turns | 2 | 2 | 2 | 2 |
| Locked groove radius | 162.5 | 149.5 | 122.0 | 101.7 |
| Silent groove pitch, px/turn | 3.2875 | 3.2875 | 2.2884 | 1.9032 |
| Pixels per mm | 3.2875 | 3.2875 | 2.2884 | 1.9032 |

The silent groove pitch equals the pixels-per-mm figure, because the band is cut at
1.00 mm per turn by definition. The run-out gaps are also physical: 3.2 mm at
the inside and 5 mm at the feed on every profile. The four profiles render to
one canvas at four scales, so a gap fixed in pixels would give four distances.
The silent groove header is 6 turns, which is 6 mm on every profile for the same
reason.

### Canvas mapping

The rendered disc fills the canvas. `outer_radius_px` is **287** for every
profile, so every format draws at one size at every physical diameter. **The
label-to-disc ratio** therefore carries the format identity on screen, because a
near-constant label occupies more of a smaller disc:

| Profile | Label radius | Ratio of canvas | Label / disc | Payload band |
| --- | --- | --- | --- | --- |
| `single45` | 151 px | 0.5243 | **52.7 %** | 177 -> 280 px (103 px) |
| `ten` | 114 px | 0.3958 | **39.9 %** | 138 -> 279 px (141 px) |
| `lp` | 95 px | 0.3299 | **33.2 %** | 115 -> 281 px (166 px) |

Radii are scaled from each profile's own physical geometry and rounded to whole
pixels:

```
scale        = outer_radius_px / (finished_diameter_mm / 2)
feature_px   = round((feature_mm / 2) * scale)
```

Reversing that rounding recovers the physical label to within 0.4 mm: 91.9 mm,
99.6 mm and 99.8 mm, against plant targets of 92 mm, 100 mm and 100 mm. The
drawn proportions therefore match the physical object, and a UI can use the
ratio above as the glyph for a format.

`margin_radius` lands on 283 px for all three profiles, so every format shares
an `outer_rim_thickness` of 4 px. Every radius below the margin varies by
profile. `lead_in_band_thickness` is the gap between the margin and the scaled
outer recorded diameter, at 6 px, 7 px and 5 px. `payload_outer_radius`
therefore lands at 280 px, 279 px and 281 px. The inner edge moves further, and
it follows the minimum inside diameter of recording rather than the label. The
label clearance below it is the difference between the two, at 26 px, 24 px and
20 px.

## Record attestation

A pressed record is immutable, and its signature covers that state. The release
commitment covers the audio and every statement the record makes about itself:
title, artist, label, catalogue number, copyright, credit, the palette, and the
geometry it was cut at. A new title or a new attribution on a signed record
therefore produces a different release.

Four items may arrive after the press. Each item arrives signed:

- **A chain anchor, an ISRC, a barcode.** Each is issued after the press: an
  anchor needs a commitment to anchor, and registrars assign ISRCs and barcodes
  on their own schedule. They sit outside the release commitment, and writing
  any of them requires the deferred attestation that signs them, bound to the
  record they belong to.
- **The sidecar.** It is meant to be rewritten — editions are issued, labels
  are re-authored — so it carries its own attestation, replaced each time it
  is written. Whoever writes it signs what they wrote.

The artist, a platform, or both may sign a release. Each party adds a separate
signature over one commitment. `record-verify` and its caller resolve the owner
of a key, outside the wire format.

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

This repository is the public reference implementation. The commercial
Bitneedle record authoring platform is a separate product. Use of the applicable crates
under the Wavey Artist Source Licence is limited to Artists and Artist
Entities. Record labels, platforms, and other commercial users need a separate
license. Refer to section 15 of `LICENSE-ARTIST`.
