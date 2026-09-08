# Tone clock (toned-v2)

A toned-v1 groove is cut in one tone, lightened across the track gaps. A **tone
clock** divides the disc into equal angular slots, from eighths to sixty-fourths.
Each slot is a pocket, and each pocket is cut in its own tone. The wheel takes
any rotation. Adjacent pockets blend at their boundary, which gives a continuous
surface across the wheel.

The app runs its colour picker once per slot of the artwork, and it passes the
resulting list to the renderer.

## Render options

| option | meaning |
|---|---|
| `grooveToneSlots` | CSS hex colours, one per slot, 2–64 of them. Slot 0 begins at the rotation and the rest follow clockwise. Setting this selects toned-v2 and takes precedence over `grooveToneColor`. |
| `grooveToneRotationDegrees` | Where slot 0 begins, in degrees clockwise from twelve o'clock. Default `0`. Stored in hundredths of a degree. |
| `grooveToneBlend` | `true` (default) lets pockets run into each other at their boundaries; `false` cuts hard edges. |
| `gapToneLightness` | As for toned-v1; applied per slot, so every pocket has a track tone and a lighter gap tone. |

Angles are in the **record's frame**: twelve o'clock is where the groove
starts (`DEFAULT_START_ANGLE`), and the wheel turns with the disc.

## Carrier cost

Bits per pixel comes from the size budget alone, as `ceil(24 / 1.2) = 20`. The
tone leaves this value unchanged, so every pocket palette holds 2^20 colours and
the bit stream runs continuously across the pockets. A clock-toned groove
therefore has the length of a single-tone cut. It is one or two pixels shorter,
because each toned-v1 span pads its own tail and the one clock stream pads once.
The descriptor holds the extra bytes: 7 bytes, plus 8 bytes per slot, plus the
gap switches. That total is about 140 bytes for sixteen pockets.

## Pocket selection

The record holds the wheel and no per-pixel value. The encoder and the decoder
walk the same spiral, so both derive the raster position of groove pixel *i*.
Both then derive its angle θ about the centre, clockwise from twelve o'clock.
`record_groove::pixel_angle` and `bytes2rgb::pixel_angle` compute that angle.

```
width    = 2π / slot_count
position = ((θ − rotation) mod 2π) / width      # in units of slots
slot     = floor(position)
if blend:
    toward_edge = position − slot − 0.5         # 0 at the centre, ±½ at the edges
    if blend_unit(i) < |toward_edge|:
        slot = neighbour on that side
gap      = odd number of gap_switch_offsets ≤ (i · bits_per_pixel) / 8
palette  = slots[slot].{base | gap_base}, at the shared bits_per_pixel
```

`blend_unit(i)` is the splitmix64 finalizer over the pixel index, mapped to
`[0, 1)`. This function is frozen. A change to it makes every blended record
already cut unreadable.

Blending selects between two existing palettes. Every toned pixel is noise, and
the eye reads the *mean* of a palette. Mixing two palettes pixel by pixel, with
a weight that rises linearly from 0 at the centre of a pocket to ½ at its edge,
therefore gives a surface whose local mean moves from one tone to the next. The
decoder recomputes the palette of each pixel from the pixel index.

The choice depends on `atan2` over integer raster coordinates, and on
`rem_euclid`. Two platforms can classify a pixel differently when the centre of
that pixel lies within one floating-point ulp of a pocket boundary. At about
10⁵ pixels, the probability is of the order of 10⁻¹⁰ per record. Such a pixel
makes the decode fail with a colour that is absent from the palette.

## Wire format

`SEGMENT_TONE_CLOCK_MAP` (32), payload encoding `toned-v2` (code 2):

```
version           u8     = 1
bits_per_pixel    u8
ordering          u8     0 = base proximity, 1 = chroma proximity
blend             u8     0 | 1
rotation          u16be  hundredths of a degree clockwise from twelve, < 36000
slot_count        u8     2..=64
slots             slot_count × (base[3], luma_tolerance, gap_base[3], gap_luma_tolerance)
gap_switch_count  varuint
gap_switch_offset varuint × gap_switch_count   strictly increasing byte offsets, none zero
```

toned-v2 requires this segment and forbids the toned-v1 carrier map. `rgb` and
`toned-v1` forbid this segment. Both identity preimages, the signed release and
the cache key, hold the map under tag 17. The encoder appends the map when the
record carries a clock, so a record with a single tone keeps the identity that
it already had.

## Cost

Per pocket: two `balanced` auto-tunes (track and gap tone, ~35 ms each
natively after the enumeration speedups) plus two palette builds (~20 ms
each), done one palette at a time over all the pixels that use it, so only
one palette is resident however many pockets there are. Measured on the
full Westside LP, 8 cores:

| wheel | cut | decode |
|---|---|---|
| 8 pockets, hard | 1.7 s | 0.5 s |
| 16 pockets, blended | 2.9 s | 1.1 s |

Single-threaded wasm takes three to four times these figures. The gap tone is
tuned separately from the track tone. A lightened tone sits nearer the gamut
ceiling, where the luma window of the track tone can hold fewer than 2^20
colours. toned-v1 has the same failure for such bases.

## Code

- `record-groove/src/clock.rs`, mirrored in `bytes2rgb/src/clock.rs` for the
  open decoder: `ToneClock`, `ClockSlot`, `pixel_angle`,
  `encode_toned_clock`, `decode_toned_clock`.
- `record-descriptor`: `ToneClockDescriptor`, `encode_tone_clock_map`,
  `decode_tone_clock_map`, `validate_tone_clock`.
- `record-render::groove_clock_track` resolves the wheel. The paint step tones
  each pixel from the pocket that the pixel falls in.
- `record-decode::decode_clock_toned_track_to_bytes` walks the same spiral
  and reverses it.
- `record-render/examples/clock_frame.rs` cuts and decodes a whole LP:
  `cargo run --release -p record-render --example clock_frame -- 16 11.25 blend`.
