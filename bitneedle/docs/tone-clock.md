# Tone clock (toned-v2)

A toned-v1 groove is cut in one tone (lightened across track gaps). A
**tone clock** divides the disc into equal angular slots — eighths,
sixteenths, up to sixty-four — each cut in its own tone, the way a roulette
wheel is divided into pockets. The wheel can be spun to any rotation, and
neighbouring pockets can blend into one another so the surface reads as a
continuum rather than a stepped pie.

The app's job is small: run its colour picker once per slot of the artwork
rather than once over the whole record, and hand the list over.

## Render options

| option | meaning |
|---|---|
| `grooveToneSlots` | CSS hex colours, one per slot, 2–64 of them. Slot 0 begins at the rotation and the rest follow clockwise. Setting this selects toned-v2 and takes precedence over `grooveToneColor`. |
| `grooveToneRotationDegrees` | Where slot 0 begins, in degrees clockwise from twelve o'clock. Default `0`. Stored in hundredths of a degree. |
| `grooveToneBlend` | `true` (default) lets pockets run into each other at their boundaries; `false` cuts hard edges. |
| `gapToneLightness` | As for toned-v1; applied per slot, so every pocket has a track tone and a lighter gap tone. |

Angles are in the **record's frame**: twelve o'clock is where the groove
starts (`DEFAULT_START_ANGLE`), and the wheel turns with the disc.

## Why it costs nothing in carrier

Bits per pixel comes from the size budget alone (`ceil(24 / 1.2) = 20`),
never from the tone, so every pocket's palette holds 2^20 colours and the
bit stream runs continuously across pockets. A clock-toned groove is the
same length as a single-tone cut — a pixel or two shorter, in fact, because
toned-v1 spans each pad their own tail and the clock's one stream pads once.
The only extra bytes are the descriptor's: 7 + 8 per slot + the gap
switches, about 140 bytes for sixteen pockets.

## How a pixel finds its pocket

Nothing per pixel is written down. Both encoder and decoder walk the same
spiral, so both know, for groove pixel *i*, the raster position it occupies
and therefore its angle θ about the centre (clockwise from twelve, computed
by `record_groove::pixel_angle` / `bytes2rgb::pixel_angle`).

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

`blend_unit(i)` is splitmix64's finaliser over the pixel index, mapped to
`[0, 1)`. It is frozen: changing it would make every blended record already
cut unreadable.

Blending never creates a new palette. Every toned pixel is noise and the
eye reads a palette's *mean*, so mixing two palettes pixel by pixel with a
weight that rises linearly from 0 at a pocket's centre to ½ at its edge
gives a surface whose local mean glides from one tone to the next — and the
decoder recomputes exactly which palette each pixel used from its index.

The choice depends on `atan2` of integer raster coordinates and on
`rem_euclid`. A pixel whose centre lies within a floating-point ulp of a
pocket boundary could in principle be classified differently by two
platforms' `atan2`; with ~10⁵ pixels the odds are on the order of 10⁻¹⁰ per
record. Decode fails loudly (colour not in palette) rather than silently.

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

toned-v2 requires this segment and forbids the toned-v1 carrier map; `rgb`
and `toned-v1` forbid it. The map is inside both identity preimages (signed
release and cache key) under tag 17 — appended only when present, so every
record without a clock keeps the identity it already had.

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

Single-threaded wasm is roughly three to four times that. The gap tone is
tuned separately from the track tone because a lightened tone sits nearer
the gamut ceiling, where the track tone's luma window can hold fewer than
2^20 colours — a latent failure toned-v1 shares for such bases.

## Code

- `record-groove/src/clock.rs`, mirrored in `bytes2rgb/src/clock.rs` for the
  open decoder: `ToneClock`, `ClockSlot`, `pixel_angle`,
  `encode_toned_clock`, `decode_toned_clock`.
- `record-descriptor`: `ToneClockDescriptor`, `encode_tone_clock_map`,
  `decode_tone_clock_map`, `validate_tone_clock`.
- `record-render::groove_clock_track` resolves the wheel; the paint step
  tones each pixel once it knows where it landed.
- `record-decode::decode_clock_toned_track_to_bytes` walks the same spiral
  and reverses it.
- `record-render/examples/clock_frame.rs` cuts and decodes a whole LP:
  `cargo run --release -p record-render --example clock_frame -- 16 11.25 blend`.
