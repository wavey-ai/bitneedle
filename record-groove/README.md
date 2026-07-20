# record-groove

This crate converts byte streams to and from RGBA pixels. It can write the
pixels to the smallest applicable square PNG. The crate processes only bytes
and color. It does not process records, spirals, or geometry.

## Formats

The crate offers three ways to pack a byte stream into pixels, trading size
against how conspicuous the result is.

| Format | Density | Pixels for N bytes | Looks like |
|---|---|---|---|
| **RGB** | 3 bytes/pixel | `ceil(N/3)` | full-color noise |
| **Grayscale** | 1 byte/pixel | `N` | gray noise (R=G=B) |
| **Toned** | `bits_per_pixel`/8 bytes/pixel | `ceil(N*8 / bits_per_pixel)` | a chosen base tone with chroma drift |

### RGB

```rust
let rgba = record_groove::bytes_to_rgba(&bytes);
let back = record_groove::rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

RGB stores three bytes in the red, green, and blue channels of each pixel. It
sets the alpha channel to opaque. RGB is the most dense format. Its output looks
like random color noise.

### Grayscale (luma)

```rust
let rgba = record_groove::bytes_to_grayscale_rgba(&bytes);
let back = record_groove::grayscale_rgba_to_bytes(&rgba, Some(bytes.len()))?;
```

One byte per pixel with `R = G = B`. Three times larger than RGB, but reads as
a neutral gray field.

### Toned

The toned format keeps **luma (brightness) fixed**. It carries data as **chroma
drift** around a base tone. The algorithm finds each 8-bit RGB color within
`luma_tolerance` of the base luma. It sorts the colors by distance from the base
color. Then, it keeps the nearest `2^bits_per_pixel` entries. Each pixel contains
one palette index.

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
| `bits_per_pixel` | Data in each pixel, from 1 through 24 bits. The number of iso-luma colors sets the limit. |
| `ordering` | which candidates make the palette: `BaseProximity` (nearest RGB distance) or `ChromaProximity` (nearest hue — see below) |

The decoder must use the **same** `TonedConfig`. This configuration defines the
palette. Both ordering methods are deterministic. Thus, the decoder can rebuild
the palette and recover the exact byte stream.

### Balanced (auto-tuned)

`TonedConfig::balanced` picks the luma tolerance that best trades brightness
drift against color cast for a given base tone and size budget:

```rust
// Best-balanced pink palette within a 1.2x size budget.
let palette = TonedPalette::balanced([0xff, 0xc0, 0xcb], 1.2)?;
let config  = palette.config(); // ordinary TonedConfig; decode side rebuilds from this
```

It sets `bits_per_pixel` to the smallest value that fits the budget. Then, it
searches a tolerance ladder that minimizes
`chroma error of the palette's mean color + luma_tolerance / 8`.

## Size vs. luma tolerance

There is a hard trade-off between output size and brightness flatness. Capacity
per pixel is `log2(number of colors sharing the base tone's luma)`. Measured
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

- **±2 / 18 bits / 1.33×** — tightest genuinely flat-brightness setting.
- **±16 / 21 bits / 1.14×** — brightness varies ≤6%, recovers most of the size gap.
- **±64 / 22 bits / 1.09×** — near RGB size, but reads as pastel static.

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

Each additional bit per pixel decreases the image size by approximately 5%.
However, it decreases tint accuracy or brightness uniformity. Payload entropy
has a larger effect on the PNG byte length. Thus, size usually refers to canvas
dimensions in this comparison.

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

## Square PNG output

```rust
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
