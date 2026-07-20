# Bitneedle Picture Record Format

A compact visual guide to the canonical **BRD1 + BRS1 + optional BSC1** format.

This README gives explanatory information. The normative specification is
`draft-bitneedle-picture-record-format-02.txt`.

## At a glance

```text
576 × 576 PNG/RGBA
┌──────────────────────────────────────────────────────────┐
│ Outer metadata band                                     │
│   BRD1 begins here                                      │
│  ┌────────────────────────────────────────────────────┐  │
│  │ Main visible groove                               │  │
│  │   direct RGB pixels → exact BRS1 bytes            │  │
│  │                                                    │  │
│  │      ┌──────────────────────────────────────┐      │  │
│  │      │ Inner metadata band                 │      │  │
│  │      │   BRD1 continues here               │      │  │
│  │      │                                      │      │  │
│  │      │ Label / optional BSC1 carrier pixels │      │  │
│  │      └──────────────────────────────────────┘      │  │
│  └────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

## Format stack

| Layer | Magic | Purpose | Structural encoding |
|---|---|---|---|
| Raster | PNG | Exact RGBA carrier | 576 × 576, 8-bit RGBA |
| Descriptor | `BRD1` | Geometry, BRS1 length, release display metadata | Binary segments |
| Record stream | `BRS1` | Payload descriptors, entries, tracks, chunks | Compact binary |
| Payload | varies | ECDC, RAW, MOSSNANO, extension container | Container-specific |
| Sidecar | `BSC1` | Text, JSON, images, opaque extras | Binary item container |

## Data flow

```text
PNG bytes
   │
   ▼
decode exact RGBA
   │
   ├── trace outer + inner metadata spirals
   │          │
   │          ▼
   │        BRD1
   │          │
   │          ├── record profile
   │          ├── b_value
   │          ├── exact BRS1 byte length
   │          ├── optional display metadata
   │          ├── optional signed-release reference
   │          └── optional BSC1 pointer
   │
   └── trace main groove using b_value
              │
              ▼
          RGB → bytes
              │
              ▼
             BRS1
              │
              ├── compact binary metadata
              ├── CRC-protected chunks
              ├── optional fixed-slot signatures
              ├── optional encryption
              └── reconstructed logical payload entries
```

## Record profiles

| Profile | Spindle radius | Label radius | Payload inner | Payload outer | Outer radius |
|---|---:|---:|---:|---:|---:|
| `single45` | 63 | 151 | 169 | 280 | 287 |
| `lp` | 7 | 95 | 109 | 280 | 287 |

```text
radius 0                                                     radius 287
   │                                                             │
   ├── spindle ── label ── trailer ── payload groove ── header ──┤
```

## BRD1 structure

### Prefix

```text
0               4 5      7      9      11                 19
┌────────────────┬─┬──────┬──────┬───────┬──────────────────┐
│ "BRD1"         │v│ total│ count│stream │ b_value bits     │
│ 4 bytes        │1│ u16  │ u16  │ u16   │ f64be / u64be   │
└────────────────┴─┴──────┴──────┴───────┴──────────────────┘
```

### Segment framing

```text
┌───────────┬──────────────┬─────────────────────────────┐
│ type: u8  │ length: u16  │ payload: length bytes       │
└───────────┴──────────────┴─────────────────────────────┘
```

### Registered segments

| Type | Field | Encoding | Required |
|---:|---|---|:---:|
| 1 | Descriptor CRC-32 | `u32be` | yes |
| 2 | Exact BRS1 length | `u64be` | yes |
| 4 | Record profile | UTF-8 | yes |
| 5 | Title | UTF-8 | no |
| 6 | Artist | UTF-8 | no |
| 7 | Payload encoding | UTF-8 (`rgb`) | yes |
| 8 | Release ID | UTF-8 | no |
| 9 | Catalog number | UTF-8 | no |
| 10 | Label | UTF-8 | no |
| 11 | Artwork credit | UTF-8 | no |
| 13 | Canonical URL | UTF-8 | no |
| 14 | Created at | UTF-8 | no |
| 16 | Signed-release reference | binary | no |
| 21 | BSC1 pointer | binary | no |

## Signed-release reference

The manifest is external. BRD1 carries only the digest and signature envelope.

```text
┌─────────┬───────────┬──────────┬───────────────┐
│ version │ hash algo │ hash len │ hash bytes    │
│ u8      │ u8        │ u16be    │ N             │
├─────────┴───────────┴──────────┴───────────────┤
│ signature algo: u8                             │
├──────────────────┬─────────────────────────────┤
│ key ID len: u16  │ opaque key ID bytes         │
├──────────────────┼─────────────────────────────┤
│ signature len    │ raw signature bytes         │
└──────────────────┴─────────────────────────────┘
```

| What BRD1 knows | What an external signing profile defines |
|---|---|
| Digest algorithm code | Meaning of that code |
| Raw manifest digest | Manifest serialization |
| Signature algorithm code | Signing preimage |
| Opaque key ID | Key resolution and revocation |
| Raw signature | Trust policy |

## BRS1 top-level layout

```text
0              4              8
┌───────────────┬──────────────┬────────────────────────────┐
│ "BRS1"        │ metadata len │ compact binary metadata    │
│ 4 bytes       │ u32be        │ N bytes                    │
├───────────────┴──────────────┴────────────────────────────┤
│ chunk 0                                                   │
├───────────────────────────────────────────────────────────┤
│ chunk 1                                                   │
├───────────────────────────────────────────────────────────┤
│ ...                                                       │
└───────────────────────────────────────────────────────────┘
```

## Compact metadata

```text
┌─────────┬───────┬──────────────────────────────────────┐
│ version │ flags │ descriptor count + descriptors       │
│ u8      │ u8    │ u8 + variable                        │
├─────────┴───────┼──────────────────────────────────────┤
│ entry count     │ entry lengths + optional indexes     │
│ u16be           │ variable                             │
├─────────────────┼──────────────────────────────────────┤
│ track count     │ titles + optional entry mappings     │
│ u16be           │ variable                             │
└─────────────────┴──────────────────────────────────────┘
```

### Metadata flags

| Bit | Meaning |
|---:|---|
| `0x01` | Chunks are encrypted and include 12-byte nonces |
| `0x02` | Payload entries explicitly store descriptor indexes |
| `0x04` | Tracks explicitly store payload-entry mappings |

### Payload descriptor

```text
container code: u8
flags:          u8
[codec string]
[sample rate: u32be]
[channels: u8]
```

| Code | Container |
|---:|---|
| 0 | RAW |
| 1 | ECDC |
| 2 | MOSSNANO |
| 255 | Extension name follows |

### Payload entries

Only lengths and optional descriptor indexes are stored.

```text
entry 0: [length varuint] [optional descriptor u8]
entry 1: [length varuint] [optional descriptor u8]
entry 2: [length varuint] [optional descriptor u8]
```

Offsets are derived:

| Entry | Stored length | Derived offset |
|---:|---:|---:|
| 0 | `L0` | `0` |
| 1 | `L1` | `L0` |
| 2 | `L2` | `L0 + L1` |
| n | `Ln` | `sum(L0..L(n-1))` |

This guarantees a contiguous logical payload:

```text
┌──────────── entry 0 ────────────┬──── entry 1 ────┬── entry 2 ──┐
0                                L0               L0+L1          total
```

### Tracks

```text
title length: u16be
title:        UTF-8
[entry index: canonical varuint]
```

Track numbers are array positions plus one.

```text
Track 1 ───────────────► payload entry 0
Track 2 ───────────────► payload entry 1
Track 3 ───────────────► payload entry 4
```

## Chunk structure

The signature slot is always physically present. Signing is optional.

### Unencrypted

```text
0    2    4 5       9       13                    77
┌────┬────┬─┬────────┬────────┬────────────────────┬──────────────┐
│idx │cnt │d│ length │ CRC-32 │ signature slot     │ payload      │
│u16 │u16 │8│ u32be  │ u32be  │ 64 bytes           │ N bytes      │
└────┴────┴─┴────────┴────────┴────────────────────┴──────────────┘
```

### Encrypted

```text
┌──────── fixed 77-byte header ────────┬────────────┬─────────────┐
│ index/count/descriptor/len/CRC/sign   │ nonce 12   │ ciphertext  │
└───────────────────────────────────────┴────────────┴─────────────┘
```

### Integrity and authorization

| Mechanism | Required? | Purpose |
|---|:---:|---|
| CRC-32 | yes | Detect stored-payload corruption |
| Signature slot | yes, physically | Stable framing |
| Non-zero signature | no | Cryptographic authorization under external profile |
| Encryption | no | Payload confidentiality and authentication |

```text
signature == 64 zero bytes  → unsigned
signature != 64 zero bytes  → signed; verify under selected profile
```

CRC success does not prove authorization.

## Single-track is not a special format

```text
Single track
  descriptors: 1
  entries:     1
  tracks:      1

Two tracks
  descriptors: 1
  entries:     2
  tracks:      2

Mixed containers
  descriptors: 2+
  entries:     N
  tracks:      M
```

```text
BRS1 payload bytes
┌─────────────────────┬──────────────────────┬───────────────────┐
│ ECDC main mix       │ ECDC instrumental    │ RAW auxiliary     │
│ entry 0 / desc 0    │ entry 1 / desc 0     │ entry 2 / desc 1  │
└─────────────────────┴──────────────────────┴───────────────────┘
```

## BSC1 remains pair-sign encoded

BSC1 is not groove patternisation. It is an independent auxiliary carrier.

```text
BRD1 segment 21
   │
   ├── scheme ID
   ├── carrier flags
   ├── shuffle seed
   ├── exact BSC1 length
   └── SHA-256 digest
          │
          ▼
selected label / intergroove / deadwax pixel pairs
          │
          ▼
deterministic shuffle
          │
          ▼
pair-sign + magnitude bits
          │
          ▼
exact BSC1 bytes
```

### Pointer

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 4 | `BSC1` |
| 4 | 1 | Version |
| 5 | 1 | Pair-sign scheme |
| 6 | 1 | Carrier flags |
| 7 | 1 | Reserved |
| 8 | 4 | Shuffle seed |
| 12 | 4 | Exact sidecar length |
| 16 | 32 | Raw SHA-256 |

### Container

```text
┌────────┬─────────┬───────┬────────────┐
│ BSC1   │ version │ flags │ item count │
│ 4      │ u8      │ u8    │ u16be      │
├────────┴─────────┴───────┼────────────┤
│ total length: u32be      │ item 0 ... │
└──────────────────────────┴────────────┘
```

### Sidecar item types and codecs

| Type code | Item |
|---:|---|
| 0 | Opaque bytes |
| 1 | UTF-8 text |
| 2 | Image |
| 3 | JSON |

| Codec code | Codec |
|---:|---|
| 0 | Raw |
| 1 | Brotli |
| 2 | Zstandard |
| 3 | AVIF |

JSON here is content inside an item, not structural BSC1 metadata.

## Complete decoder checklist

```text
[1] PNG → exact RGBA
[2] profile → single45 or lp
[3] metadata spirals → BRD1
[4] BRD1 CRC and required segments
[5] main groove → RGB bytes
[6] truncate to exact BRS1 length
[7] compact binary metadata
[8] chunk framing and CRC checks
[9] classify optional signatures
[10] optional signature verification
[11] optional chunk decryption
[12] concatenate payload bytes
[13] resolve entry offsets
[14] resolve track mappings
[15] optionally recover BSC1
```

## What was removed

| Removed experiment | Canonical replacement |
|---|---|
| JSON BRS1 metadata | Compact binary metadata |
| `BCS2` | `BRS1` |
| `BPLP` / `ECLP` wrappers | Native payload entries |
| Unwrapped single payload | One-entry BRS1 |
| `BTS1` | `BSC1` |
| `toned-v1` / `rgbTone` | Direct RGB |
| Groove patternisation | Removed |
| `BNPM` reverse map | Removed |
| Inline JSON release manifest | Binary external-manifest reference |
| JSON authorization and receipts | External profiles |

## Human-readable inspection

Binary wire formats should still be easy to inspect.

`test-spin` decodes typed structures and may render them as pretty JSON:

```text
BRD1/BRS1 binary bytes
        │
        ▼
typed Rust structures
        │
        ├── labeled terminal report
        ├── pretty JSON diagnostic view
        └── raw hexadecimal prefix
```

The diagnostic JSON is not canonical wire data.
