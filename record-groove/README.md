# record-groove

Colour-space utilities for bitneedle: converting byte streams to and from RGBA
pixels, and writing them as best-fit square PNGs. Nothing in this crate knows
about records, spirals, or geometry — it only deals with bytes and colour.

## Formats

The crate offers three ways to pack a byte stream into pixels, trading size
against how conspicuous the result is.

| Format | Density | Pixels for N bytes | Looks like |
|---|---|---|---|
| **RGB** | 3 bytes/pixel | `ceil(N/3)` | full-colour noise |
| **Grayscale** | 1 byte/pixel | `N` | gray noise (R=G=B) |
| **Toned** | `bits_per_pixel`/8 bytes/pixel | `ceil(N*8 / bits_per_pixel)` | a chosen base tone with chroma drift |

### RGB

```rust
let rgba = record_groove::bytes_to_rgba(&bytes);
let back = record_groove::rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

Three bytes per pixel into R/G/B, alpha forced opaque. Densest format; the
output is indistinguishable from random colour noise.

### Grayscale (luma)

```rust
let rgba = record_groove::bytes_to_grayscale_rgba(&bytes);
let back = record_groove::grayscale_rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

One byte per pixel with `R = G = B`. Three times larger than RGB, but reads as
a neutral gray field.

### Toned

The toned format keeps **luma (brightness) fixed** and carries data as **chroma
drift** around a base tone. It builds a palette of every 8-bit RGB colour whose
rounded Rec. 709 luma matches the base tone's (within `luma_tolerance`), sorts
that palette by closeness to the base, keeps the nearest `2^bits_per_pixel`
entries, and encodes each pixel as a palette index.

```rust
use record_groove::{TonedConfig, TonedPalette, rgba_to_square_png};

// Pink base, brightness held within ±2 luma steps, 18 bits per pixel.
let config = TonedConfig::from_hex("#FFC0CB", 2, 18);
let palette = TonedPalette::from_config(config)?;

let rgba = palette.bytes_to_rgba(&bytes);
let png  = rgba_to_square_png(&rgba)?;
let back = palette.rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

Everything is configurable through `TonedConfig`:

| Field | Meaning |
|---|---|
| `base` | the `[u8; 3]` base tone every pixel's brightness matches (or use `TonedConfig::from_hex`) |
| `luma_tolerance` | allowed brightness drift in rounded Rec. 709 luma steps; `0` = perfectly flat |
| `bits_per_pixel` | data packed per pixel (1–24); bounded by how many iso-luma colours exist |
| `ordering` | which candidates make the palette: `BaseProximity` (nearest RGB distance) or `ChromaProximity` (nearest hue — see below) |

The decoder must be built with the **same** `TonedConfig` — the palette is the
key. Both orderings are deterministic functions of the config, so rebuilding
the palette on the decode side always recovers the byte stream exactly.

### Balanced (auto-tuned)

`TonedConfig::balanced` picks the luma tolerance that best trades brightness
drift against colour cast for a given base tone and size budget:

```rust
// Best-balanced pink palette within a 1.2x size budget.
let palette = TonedPalette::balanced([0xff, 0xc0, 0xcb], 1.2)?;
let config  = palette.config(); // ordinary TonedConfig; decode side rebuilds from this
```

It fixes `bits_per_pixel` to the smallest count that fits the budget, then
searches a tolerance ladder minimising
`chroma error of the palette's mean colour + luma_tolerance / 8`.

## Size vs. luma tolerance

There is a hard trade-off between output size and brightness flatness. Capacity
per pixel is `log2(number of colours sharing the base tone's luma)`. Measured
for a pink base (`#FFC0CB`, luma ≈ 206):

| Luma tolerance | Iso-luma colours | Max bits/pixel | Size vs RGB |
|---|---|---|---|
| ±0 (flat) | 67,783 | 16 | 1.50× |
| ±2 | 338,901 | 18 | 1.33× |
| ±4 | 610,026 | 19 | 1.26× |
| ±8 | 1,151,582 | 20 | 1.20× |
| ±16 | 2,213,246 | 21 | 1.14× |
| ±32 | 4,098,936 | 21 | 1.14× |
| ±64 | 7,105,749 | 22 | 1.09× |
| ±128 | 12,970,272 | 23 | 1.04× |
| ±255 | 16,777,216 | 24 | 1.00× |

Reaching **1.00× (RGB density)** requires the entire 16.7M-colour cube, i.e.
`luma_tolerance = 255` — which is no brightness constraint at all, identical to
the plain RGB format. **You cannot beat RGB density while constraining luma; the
two are the same knob.** As tolerance rises the palette also spreads across the
hue wheel, so the *average* colour drifts away from the base tone toward
desaturated full-spectrum noise — flat brightness and a recognisable tint pull
in opposite directions.

Practical operating points:

- **±2 / 18 bits / 1.33×** — tightest genuinely flat-brightness setting.
- **±16 / 21 bits / 1.14×** — brightness varies ≤6%, recovers most of the size gap.
- **±64 / 22 bits / 1.09×** — near RGB size, but reads as pastel static.

## Colour cast: why everything trends green, and the fix

With uniform (compressed/encrypted) data every palette index is equally
likely, so the rendered image averages to the **mean colour of the palette** —
no reordering or dithering can change that; only the choice of *which* colours
make up the palette can.

Rec. 709 luma is green-heavy (`Y = 0.21R + 0.72G + 0.07B`). To hit a high luma
like pink's (≈206), green is *forced* high (G ≥ ~164 even with R and B maxed)
while red and blue roam freely and average to mid — so the full iso-luma set's
mean is mint green, and a `BaseProximity` palette that nearly exhausts it
renders green regardless of the base tone. Dark base tones mirror this and
skew magenta.

The fix is `ToneOrdering::ChromaProximity`: sort candidates by Cb/Cr distance
to the base tone and keep only the nearest `2^bits_per_pixel`. This only works
when the candidate pool is *larger* than the palette — tint is bought with
selection headroom, which comes from either a wider luma window (same size,
grainier brightness) or fewer bits per pixel (flatter, larger). Measured on a
255 KB payload, pink base `#FFC0CB`, chroma ordering:

| Bits/px | Luma window | Size | Mean colour | Max drift | Look |
|---|---|---|---|---|---|
| 22 | ±64 | 1.09× | (183, 184, 148) | 245 | warm khaki |
| 22 | ±96 | 1.09× | (188, 153, 149) | 256 | dusty rose, noisy |
| 21 | ±16 | 1.14× | (162, 224, 137) | 264 | mint (no headroom: needs 2.10M of 2.15M colours) |
| 21 | ±32 | 1.14× | (194, 206, 161) | 195 | sage |
| 21 | ±48 | 1.14× | (202, 190, 169) | 185 | warm neutral |
| **21** | **±64** | **1.14×** | (205, 174, 169) | 191 | **pink-mauve — best at this size** |
| 21 | ±96 | 1.14× | (199, 150, 156) | 223 | deeper rose, darker/grainier |
| 20 | ±32 | 1.20× | (214, 196, 182) | 143 | pink-beige |
| **20** | **±48** | **1.20×** | (218, 182, 182) | 146 | **clean pink, calmest pixels** |
| 20 | ±64 | 1.20× | (216, 168, 174) | 161 | stronger rose |

Each extra bit per pixel saves ~5% size but costs either tint (greener) or
brightness flatness (wider luma window). PNG byte sizes barely move across all
of these — payload entropy dominates — so "size" is mostly canvas dimensions.

### Does one preset work for every base colour?

20 bits / ±48 / chroma ordering is feasible for every base tone tested,
including black and white, but `TonedConfig::balanced` adapts the window per
tone and matches or beats it everywhere — dark and very light tones need a
wider window (their luma slab clips against the gamut edge), grey needs almost
none. Chroma error is the Cb/Cr distance from the palette mean to the base
tone (lower is better):

| Base | `balanced(1.2)` picked | Mean colour | Chroma error |
|---|---|---|---|
| white `#FFFFFF` | ±96, 20 bits | (195, 198, 193) | 2.5 |
| black `#000000` | ±96, 20 bits | (60, 57, 62) | 2.5 |
| grey `#808080` | ±8, 20 bits | (128, 128, 128) | 0.0 |
| pink `#FFC0CB` | ±64, 20 bits | (216, 168, 174) | 7.3 |
| red `#FF0000` | ±64, 20 bits | (212, 41, 62) | 47.4 |
| orange `#FF8000` | ±48, 20 bits | (215, 133, 46) | 35.4 |
| yellow `#FFFF00` | ±64, 20 bits | (197, 220, 52) | 48.7 |
| green `#00C040` | ±48, 20 bits | (36, 201, 82) | 13.2 |
| cyan `#00C0C0` | ±48, 20 bits | (36, 195, 190) | 17.9 |
| blue `#2060C0` | ±48, 20 bits | (45, 99, 192) | 5.8 |
| navy `#101840` | ±64, 20 bits | (49, 51, 92) | 3.0 |
| lilac `#C8B4E6` | ±48, 20 bits | (187, 171, 208) | 6.2 |

Saturated primaries (red, yellow, orange) sit at gamut corners where few
colours share their chroma, so some cast is unavoidable at any setting.
Pastels, muted tones, and anything mid-saturation reproduce well (error 3–18).

## Square PNG output

```rust
let png  = record_groove::rgba_to_square_png(&rgba)?;   // smallest fitting square
let rgba = record_groove::square_png_to_rgba(&png)?;     // round-trips exactly
```

Pads the unused tail of the square with fully transparent pixels, which every
decoder above skips, so the padding is invisible to a round-trip.

## Licence

This crate is source-available under the Wavey Artist Source Licence.
Individual artists and artist-controlled entities may use it free of charge to
create and sell records containing their own work.
Record labels, platforms, hosted services, technology providers, and other
commercial users require a separate licence from Wavey, Inc.
Commercial licensing: licence@yl.vin
This crate is not open-source software.
