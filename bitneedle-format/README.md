# Bitneedle Picture Record Format

A compact visual guide to the canonical **BRD1 + BRS1 + optional BSC1** format.

BPK1 transports these exact components before PNG rendering.

This README gives explanatory information. The normative specification is
`draft-bitneedle-picture-record-format-03.txt`.

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
| Package | `BPK1` | Optional transport for exact record components | Binary directory and component bytes |
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
│ "BRD1"         │v│ total│ segs │ seg   │ b_value bits     │
│ 4 bytes        │2│ u16be│ u16be│ bytes │ f64be / u64be    │
└────────────────┴─┴──────┴──────┴───────┴──────────────────┘
```

`total` is the whole descriptor payload including this 19-byte prefix, and
must equal `19 + seg bytes`. `segs` is the number of segments that follow,
and a reader that parses a different number fails. `b_value` must decode
finite and positive. The current version is `2`; a reader refuses any other.

### Segment framing

```text
┌───────────┬──────────────┬─────────────────────────────┐
│ type: u8  │ length: u16  │ payload: length bytes       │
└───────────┴──────────────┴─────────────────────────────┘
```

### Registered segments

| Type | Field | Encoding | Bytes | Required |
|---:|---|---|---|:---:|
| 1 | Descriptor CRC-32 | `u32be`, zeroed while hashing | 4 | yes |
| 2 | Exact BRS1 length | `u32be`, non-zero | 4 | yes |
| 4 | Record profile | profile code (`0` single45, `1` lp) | 1 | yes |
| 5 | Title | UTF-8 | ≤ 96 | no |
| 6 | Artist | UTF-8 | ≤ 96 | no |
| 7 | Payload encoding | encoding code (`0` rgb, `1` toned-v1) | 1 | yes |
| 8 | Release ID | raw ULID bytes | 16 | no |
| 9 | Catalog number | UTF-8 | ≤ 96 | no |
| 10 | Label | UTF-8 | ≤ 96 | no |
| 11 | Artwork credit | UTF-8 | ≤ 96 | no |
| 13 | Canonical URL | UTF-8 | ≤ 96 | no |
| 14 | Created at | `u64be` seconds | 8 | no |
| 16 | Signed-release reference | binary, see below | variable | no |
| 21 | BSC1 pointer | binary, non-empty | variable | no |
| 22 | Toned carrier map | binary, see below | variable | with `toned-v1` |
| 23 | Cache encryption | binary, see below | 4 + 32 | no |
| 24 | Copyright year | `u16be` | 2 | no |
| 25 | Copyright holder | UTF-8 | ≤ 96 | no |
| 26 | Chain anchor | binary, non-empty | variable | deferred |
| 27 | ISRCs | count + `(track u16be, 12 ASCII)` | variable | deferred |
| 28 | Barcode | 12–14 ASCII digits, mod-10 checked | 12–14 | deferred |
| 29 | Deferred attestation | signature envelope | variable | with 26–28 |
| 31 | Additional signatures | count + length-prefixed envelopes | variable | no |

Types 3, 12, 15, 17–20, 30 and 32 upward are unallocated. A text segment written
with an empty payload is indistinguishable from an absent one: the encoder
omits both.

### Unknown segments

A reader **skips** any segment type it does not know, and keeps parsing.
Segments are type-length-value, so stepping over one is exact, and the
descriptor CRC-32 covers the whole payload — unknown bytes included — so
corruption is still caught. A reader **must** fail on a CRC mismatch.

This is what makes the segment table additive: a writer may add a type
without making its records unreadable by readers built before it. Such a
reader drops the unknown segment if it re-authors the descriptor, so a
writer must not treat a round trip through an older reader as lossless.

## Signed-release reference

The manifest is external. BRD1 carries only the digest and the signature
envelope.

Version 2 fixes the algorithms — SHA-256 and Ed25519 — so there are no
per-reference algorithm selectors and both lengths are constant.

```text
┌─────────┬──────────────────────────┬─────────────────┬────────────────┐
│ version │ release commitment       │ key ID len      │ key ID bytes   │
│ u8 (=2) │ SHA-256, 32 bytes        │ u16be, non-zero │ N              │
├─────────┴──────────────────────────┴─────────────────┴────────────────┤
│ Ed25519 signature, 64 bytes                                           │
└───────────────────────────────────────────────────────────────────────┘
```

The signed digest is the release commitment from `record-core`:

```text
SHA-256( "BITNEEDLE-RELEASE-V1" || release_id || record_profile_code
         || payload_encoding_code || SHA-256(BRS1 bytes)
         || revolution_count: u32be || revolution commitments… )
```

each revolution commitment being:

```text
SHA-256( "BITNEEDLE-REVOLUTION-V1" || release_id
         || revolution_index: u32be || SHA-256(ECDC entry bytes) )
```

So the signature binds five things and nothing else: the release ID, the
record profile, the payload encoding kind, the whole BRS1 stream, and every
revolution's audio in order. Every other segment — title, artist, catalogue,
label, credit, URL, dates, the BSC1 pointer, the toned carrier map, even
`b_value` — may be re-authored after pressing without invalidating it.

| What BRD1 knows | What an external signing profile defines |
|---|---|
| The release commitment digest | The manifest that produced it |
| Opaque key ID | Key resolution and revocation |
| Raw signature | Trust policy |

## What a signature covers

A pressed record is immutable. Everything it says about itself — its title,
its artist, its label and catalogue number, the copyright line, the credit,
the URL, the date, the palette and the geometry it was cut at — is inside the
release commitment. Change any of it and the commitment changes, which is
what pressing another record means.

```text
release commitment (v2)
  SHA-256( "BITNEEDLE-RELEASE-V2" || release_id
           || record_profile_code || payload_encoding_code
           || SHA-256(BRS1 bytes)
           || SHA-256(descriptor identity)      ← everything above
           || revolution_count: u32be
           || revolution commitments… )

revolution commitment
  SHA-256( "BITNEEDLE-REVOLUTION-V1" || release_id
           || revolution_index: u32be || SHA-256(ECDC entry bytes) )
```

Version 1 of the release commitment omitted the descriptor digest, so a
signed record could be re-titled and re-attributed by anyone and still
verify. It remains defined for records already pressed under it.

### The deferred group

Three fields cannot exist when the record is cut: a chain anchor needs a
commitment to anchor, and ISRCs and barcodes are issued by registrars on
their own schedule. They sit outside the release commitment for that reason
and no other — and they are never simply left open.

```text
deferred commitment
  SHA-256( "bitneedle.record-descriptor.deferred.v1"
           || SHA-256(descriptor identity)      ← binds it to this record
           || chain anchor || ISRCs || barcode )
```

Segment 29 signs that digest. Writing a chain anchor, an ISRC or a barcode
**requires** it, and an attestation with nothing deferred to sign is equally
malformed — a deferred field is null or signed, and a reader refuses anything
between. Binding to the descriptor identity, rather than to the press
signature, is what stops a signed barcode being lifted off one record and
dropped onto another, and lets a record that was never signed at press still
carry a signed one.

### More than one signer

A release may be attested by the artist, by the platform, or by both. They
are signatures over the same commitment — one digest, signed independently —
so segment 31 is a list, not a role table. Who a signature belongs to is a
question for whoever resolves its key ID; the wire format does not decide it,
and order carries no meaning.

A reader checks that every signature covers the same commitment and that no
key signs twice. Additional signatures without a primary are malformed. A
key held on the signer's behalf — an artist signing through an account rather
than from a device — produces an identical envelope; the difference lives
entirely in what the verifier decides that key means.

### The sidecar signs itself

The descriptor is immutable, so it cannot hold a signature for something that
changes. The sidecar does change: editions are issued and labels are
re-authored, and the party doing that is often not the artist, whose key may
be nowhere near the machine.

So the sidecar carries its own attestation, as a reserved item named
`attestation`, replaced along with everything else each time the sidecar is
written. Whoever writes it signs what they wrote.

```text
sidecar commitment
  SHA-256( "bitneedle.record-sidecar.attestation.v1"
           || SHA-256(descriptor identity)      ← binds it to this record
           || item count
           || for each item but the attestation:
                type, codec, name, stored length, SHA-256(stored bytes) )
```

The attestation excludes itself, because it cannot contain its own
signature. Editing any other item invalidates it, which is the point.

This is what the edition number rides in: it is issued by the platform, at a
moment when no artist key need be present, so it belongs where a signature
can be made by whoever is actually there.

### Three tiers, one record

| Signed | By | When | Covers |
|---|---|---|---|
| Release commitment (16, 31) | artist, platform, or both | at the press | the audio and everything the record says |
| Deferred attestation (29) | whoever holds a key then | when codes arrive | chain anchor, ISRCs, barcode |
| Sidecar attestation (item) | the party rewriting it | every sidecar write | press metadata, edition, images |

Verification of any of them is a matter of resolving a key ID and checking
an Ed25519 signature, which BRD1 leaves to `record-verify` and its caller.
The format states what is covered; it does not state whom to believe.

## Toned carrier map

Present only with the `toned-v1` payload encoding, and required by it.

```text
┌─────────┬─────────────┬──────────────────────────────────────────────┐
│ version │ span count  │ spans…                                       │
│ u8 (=1) │ u16be       │                                              │
└─────────┴─────────────┴──────────────────────────────────────────────┘

span: [byte length: varuint] [base RGB: 3] [luma tolerance: u8]
      [bits per pixel: u8, 1–24] [ordering: u8]
```

Ordering is `0` base-proximity or `1` chroma-proximity. Span offsets are
derived by accumulation, and the spans must cover the BRS1 length exactly.

## Cache encryption

```text
┌─────────┬───────────┬────────────────┬────────────┬──────────────────┐
│ version │ algorithm │ key derivation │ secret len │ secret bytes     │
│ u8 (=1) │ u8 (=1)   │ u8 (=1)        │ u8 (=32)   │ 32               │
└─────────┴───────────┴────────────────┴────────────┴──────────────────┘
```

Algorithm `1` is XChaCha20-Poly1305; key derivation `1` is HKDF-SHA256. The
secret is bound to the record by `cache_encryption_record_binding_hash`,
which hashes a far wider preimage than the signature does — it covers the
display metadata, the BSC1 pointer, the toned map, `b_value` and the stream
length, excluding only the signed-release reference and this segment. Editing
a field that the signature ignores will therefore still break an existing
cache binding.

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
