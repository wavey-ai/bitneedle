# record-groove

This crate converts byte streams to and from RGBA pixels. It can write the
pixels to the smallest applicable square PNG. The crate processes only bytes
and color. It does not process records, spirals, or geometry.

## Formats

The crate offers three ways to pack a byte stream into pixels. The three
formats give three points on the trade between output size and visual
appearance.

| Format | Density | Pixels for N bytes | Looks like |
|---|---|---|---|
| **RGB** | 3 bytes/pixel | `ceil(N/3)` | full-color noise |
| **Grayscale** | 1 byte/pixel | `N` | gray noise (R=G=B) |
| **Toned** | `bits_per_pixel`/8 bytes/pixel | `ceil(N*8 / bits_per_pixel)` | a chosen base tone with chroma drift |

### RGB

```rust,ignore
let rgba = record_groove::bytes_to_rgba(&bytes);
let back = record_groove::rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

RGB stores three bytes in the red, green, and blue channels of each pixel. It
sets the alpha channel to opaque. RGB is the most dense format. Its output looks
like random color noise.

### Grayscale (luma)

```rust,ignore
let rgba = record_groove::bytes_to_grayscale_rgba(&bytes);
let back = record_groove::grayscale_rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

Grayscale stores one byte per pixel, with `R = G = B`. Its output is three
times larger than RGB output, and it appears as a neutral gray field.

### Toned

The toned format keeps **luma (brightness) fixed**. It carries data as **chroma
drift** around a base tone. The algorithm finds each 8-bit RGB color within
`luma_tolerance` of the base luma. It sorts the colors by distance from the base
color. Then, it keeps the nearest `2^bits_per_pixel` entries. Each pixel contains
one palette index.

```rust,ignore
use record_groove::{TonedConfig, TonedPalette, rgba_to_square_png};

// Pink base, brightness held within ±2 luma steps, 18 bits per pixel.
let config = TonedConfig::from_hex("#FFC0CB", 2, 18);
let palette = TonedPalette::from_config(config)?;

let rgba = palette.bytes_to_rgba(&bytes);
let png  = rgba_to_square_png(&rgba)?;
let back = palette.rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

`TonedConfig` sets these four fields:

| Field | Meaning |
|---|---|
| `base` | the `[u8; 3]` base tone every pixel's brightness matches (or use `TonedConfig::from_hex`) |
| `luma_tolerance` | allowed brightness drift in rounded Rec. 709 luma steps; `0` holds the luma constant |
| `bits_per_pixel` | Data in each pixel, from 1 through 24 bits. The number of iso-luma colors sets the limit. |
| `ordering` | which candidates make the palette: `BaseProximity` (nearest RGB distance) or `ChromaProximity` (nearest hue — see below) |

The decoder must use the **same** `TonedConfig`. This configuration defines the
palette. Both ordering methods are deterministic. Thus, the decoder can rebuild
the palette and recover the exact byte stream.

### Balanced (auto-tuned)

`TonedConfig::balanced` picks the luma tolerance that best trades brightness
drift against color cast for a given base tone and size budget:

```rust,ignore
// Best-balanced pink palette within a 1.2x size budget.
let palette = TonedPalette::balanced([0xff, 0xc0, 0xcb], 1.2)?;
let config  = palette.config(); // ordinary TonedConfig; decode side rebuilds from this
```

It sets `bits_per_pixel` to the smallest value that fits the budget. Then, it
searches a tolerance ladder that minimizes
`chroma error of the palette's mean color + luma_tolerance / 8`.

## Size vs. luma tolerance

Output size and brightness flatness trade against each other. Capacity per pixel
is `log2(number of colors sharing the base tone's luma)`. Measured
for a pink base (`#FFC0CB`, luma ≈ 206):

| Luma tolerance | Iso-luma colors | Max bits/pixel | Size vs RGB |
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

The **1.00× RGB density** requires all 16.7 million colors. This value requires
`luma_tolerance = 255`, which does not constrain brightness. Thus, it is the same
as the plain RGB format. A luma constraint always decreases the RGB density.

When tolerance increases, the palette uses a larger part of the hue wheel. The
average color moves from the base tone toward desaturated full-spectrum noise.
Flat brightness and a recognizable tint require different settings.

Practical operating points:

- **±2 / 18 bits / 1.33×** — the tightest setting that holds brightness flat.
- **±16 / 21 bits / 1.14×** — brightness varies by 6% or less, and the size gap
  narrows.
- **±64 / 22 bits / 1.09×** — close to RGB size, and it appears as pastel
  static.

## Color cast

Uniform compressed or encrypted data gives the same probability to each palette
index. Thus, the rendered image approaches the **mean palette color**. Ordering
and dithering do not change this result. Only the selected palette colors can
change the mean color.

Rec. 709 luma gives the largest weight to green
(`Y = 0.21R + 0.72G + 0.07B`). A pink luma of approximately 206 requires a high
green value. Green must be at least approximately 164 when red and blue are at
their maximum values. Red and blue can use a larger range. Thus, the mean of the
complete iso-luma set is mint green. A large `BaseProximity` palette also looks
green.

Dark base tones can have a magenta cast.

Use `ToneOrdering::ChromaProximity` to decrease the color cast. It sorts
candidates by their Cb/Cr distance from the base tone. It keeps the nearest
`2^bits_per_pixel` candidates. The candidate pool must be larger than the
palette. A wider luma window gives more candidates but makes brightness less
uniform.

Fewer bits per pixel also give more candidates but increase the image
size. The following results use a 255 KB payload and the pink base `#FFC0CB`:

| Bits/px | Luma window | Size | Mean color | Max drift | Look |
|---|---|---|---|---|---|
| 22 | ±64 | 1.09× | (183, 184, 148) | 245 | warm khaki |
| 22 | ±96 | 1.09× | (188, 153, 149) | 256 | dusty rose, noisy |
| 21 | ±16 | 1.14× | (162, 224, 137) | 264 | mint (no headroom: needs 2.10M of 2.15M colors) |
| 21 | ±32 | 1.14× | (194, 206, 161) | 195 | sage |
| 21 | ±48 | 1.14× | (202, 190, 169) | 185 | warm neutral |
| **21** | **±64** | **1.14×** | (205, 174, 169) | 191 | **pink-mauve — best at this size** |
| 21 | ±96 | 1.14× | (199, 150, 156) | 223 | deeper rose, darker/grainier |
| 20 | ±32 | 1.20× | (214, 196, 182) | 143 | pink-beige |
| **20** | **±48** | **1.20×** | (218, 182, 182) | 146 | **clean pink, calmest pixels** |
| 20 | ±64 | 1.20× | (216, 168, 174) | 161 | stronger rose |

Each additional bit per pixel decreases the image size by approximately 5%. It
also decreases tint accuracy or brightness uniformity. Payload entropy has a
larger effect on the PNG byte length, so size in this comparison refers to the
canvas dimensions.

### One preset for all base colors

The 20-bit, ±48, chroma-ordering configuration works for each tested base tone.
The tests include black and white. However, `TonedConfig::balanced` adjusts the
window for each tone and gives equal or better results. Dark and very light
tones require a wider window because the gamut edge limits their luma range.

Gray requires almost no adjustment. Chroma error is the Cb/Cr distance from the
palette mean to the base tone. A lower value is better.

| Base | `balanced(1.2)` picked | Mean color | Chroma error |
|---|---|---|---|
| white `#FFFFFF` | ±96, 20 bits | (195, 198, 193) | 2.5 |
| black `#000000` | ±96, 20 bits | (60, 57, 62) | 2.5 |
| gray `#808080` | ±8, 20 bits | (128, 128, 128) | 0.0 |
| pink `#FFC0CB` | ±64, 20 bits | (216, 168, 174) | 7.3 |
| red `#FF0000` | ±64, 20 bits | (212, 41, 62) | 47.4 |
| orange `#FF8000` | ±48, 20 bits | (215, 133, 46) | 35.4 |
| yellow `#FFFF00` | ±64, 20 bits | (197, 220, 52) | 48.7 |
| green `#00C040` | ±48, 20 bits | (36, 201, 82) | 13.2 |
| cyan `#00C0C0` | ±48, 20 bits | (36, 195, 190) | 17.9 |
| blue `#2060C0` | ±48, 20 bits | (45, 99, 192) | 5.8 |
| navy `#101840` | ±64, 20 bits | (49, 51, 92) | 3.0 |
| lilac `#C8B4E6` | ±48, 20 bits | (187, 171, 208) | 6.2 |

Saturated primary colors are at the corners of the gamut. Few colors have the
same chroma in these areas. Thus, red, yellow, and orange have some color cast
at all settings. Pastels, muted tones, and medium-saturation colors give chroma
error values from 3 through 18.

## The tone clock

A clock tones the groove by *the position of a pixel on the disc*. The clock
band divides by radius into rings. Each ring divides into equal angular slots
from its own rotation. The coordinates of a pixel therefore give its pocket. The
house wheel is `[8, 16]`: eight pockets across the inside of the band, sixteen
around the outside, and twenty-four in total.

The band runs from the label edge out to the outermost groove. It therefore
holds every band the groove runs through: the programme, the silent groove, the
run-out rings and the locked groove. A radius outside the band takes the
nearest ring, so a band that stopped at the programme gave the innermost ring
to every trailer pixel.

Each pocket carries its own base tone and its own luma tolerance. The record
holds the map, which is the wheel itself, so a reader recovers the pockets from
the disc alone.

```rust,ignore
let pocket = clock.cell_index(pixel_index, angle, radius);  // radius -> ring, angle -> slot
let palette = clock.config(pocket, clock.is_gap(pixel_index));
```

The ring choice reads the radius alone. The slot choice reads the rotation of
its own ring alone. The two rings therefore turn independently.

### Unique editions

Two controls make a re-press a different record, and each control has its own
effect.

**Rotation** — one turn per ring, carried as a `u16` in hundredths of a degree.
Each pocket re-reads its colour from the art that it covers. A turn of exactly
one pocket width therefore puts the boundaries back on themselves and reproduces
the pressed record. The unique range is one pocket width per ring: 45° for the
inner ring and 22.5° for the outer ring. A turn below about a quarter of a degree
moves the boundary less than one pixel. A 45 therefore holds about 179 inner
positions and 110 outer positions.

**Base nudge** — a move of the base tone of one pocket by one 8-bit step. The
palette holds the 2²⁰ iso-luma colours *ordered by chroma proximity to the
base*, so a one-unit move re-sorts the whole ordering. Across four
representative tones, **about 100% of the million palette entries change**.
Every groove pixel therefore takes a different colour at the same brightness.
The picture holds its appearance.

In OKLab (×100), a one-step nudge measures:

| base | nudge ΔE (r/g/b) | the pocket's palette already spreads to |
|---|---|---|
| `[150,96,120]` | 0.18 / 0.27 / 0.16 | ΔE 21.6 |
| `[64,140,90]` | 0.08 / 0.31 / 0.14 | ΔE 17.4 |
| `[200,180,60]` | 0.15 / 0.26 / 0.07 | ΔE 13.6 |
| `[30,30,40]` | 0.19 / 0.36 / 0.20 | ΔE 28.0 |

The nudge is below a just-noticeable difference. It is 60 to 170 times smaller
than the spread that the colours of the pocket already cover.

### Edition capacity

The press draws each edition at random from the scheme, so two editions can
coincide. For `N` distinct records, the chance of a collision reaches 50% at
about `1.177 * sqrt(N)` pressings.

| scheme | distinct records | 50% collision at |
|---|---|---|
| rotation pair only | ~19 700 | ~165 pressings |
| + one nudge shared by every pocket (27 deltas) | ~532 000 | ~860 |
| + a per-pocket ±1 nudge on one channel (3²⁴) | ~5.5 × 10¹⁵ | **~87 million** |

Take the per-pocket nudge. The caller already chooses the twenty-four tones that
it passes in, so the format, the carrier and the renderer stay as they are.

### Nudge compatibility

`bits_per_pixel` comes from the size budget alone, as
`ceil(24 / max_size_factor)`. The ordering is always chroma proximity. Both
values are independent of the base tone, so every pocket agrees on the bit count
of a pixel under any nudge. The map carries the base and the tolerance of each
pocket, and a reader rebuilds the palette from those written values. Every wheel
that presses therefore reads.

A nudge fails when `TonedConfig::balanced` refuses the tone, which happens near
black, near white and at the gamut corners. In that case no tolerance yields the
2²⁰ colours that the budget needs. This refusal occurs at press time.

## Square PNG output

```rust,ignore
let png  = record_groove::rgba_to_square_png(&rgba)?;   // smallest fitting square
let rgba = record_groove::square_png_to_rgba(&png)?;     // round-trips exactly
```

The function fills the unused end of the square with transparent pixels. Each
decoder skips these pixels. Thus, the padding does not change a round trip.

## License

This crate is source-available under the Wavey Artist Source Licence.
Individual artists and artist-controlled entities may use it free of charge to
create and sell records containing their own work.
Record labels, platforms, hosted services, technology providers, and other
commercial users require a separate license from Wavey, Inc.
Commercial licensing: license@yl.vin
This crate is not open-source software.
