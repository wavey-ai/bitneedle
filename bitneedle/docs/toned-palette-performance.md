# Toned palette auto-tune speed and carrier overhead

Notes from a pass over `record-groove` on 2026-09-04. The pass answered two
questions: the speed of the auto-tone, and the 20% of carrier that toning costs.
The auto-tone is now four times faster. The 20% is a colour-quality setting.

## Speed

Two operations hold the toning cost: `TonedConfig::balanced`, which is the
auto-tune, and the two palette builds on the cut path, for the track tone and
the gap tone. Encoding the pixels takes under 1 ms. Measured on the house tone
`#F2EEE5`, in a release build, on an 8-core machine:

| step | before | after |
|---|---|---|
| `balanced` (auto-tune) | 207 ms | 41 ms |
| palette build (×2 on cut) | 68 ms | 21 ms |
| decode (140 KB payload) | 40 ms | 20 ms |
| **cut path total** | **~340 ms** | **~85 ms** |

Six changes produced these figures. All six produce the same palettes. The pass
fingerprinted 15 configs, across 5 tones and 3 budgets, before and after, and
the hashes match:

- **Blue range by bisection**, in place of a test of every blue value. Rounded
  luma is monotone in blue, so 8 probes on the same predicate find both ends of
  the window. This change alone takes the 9 `iso_luma_count` calls of the ladder
  to a small fraction of their earlier cost.
- **Hoisted the base tone's chroma** and passed each colour's luma through
  instead of recomputing it three times per colour.
- **One tally per colour** in the ladder histogram (under its narrowest rung,
  prefix-summed after) instead of one per rung.
- **Multi-threaded enumeration** on native (`std::thread::scope`, no new
  deps; sequential on wasm32 where there are no threads). Folds are sums or
  sorted collections, so thread order can't change the result.
- **Palette sort by bucket runs** in parallel instead of `select_nth` + one
  1M-element sort.
- **Reverse index as a sorted packed `Vec<u64>`**, in place of a SipHash
  `HashMap`. It builds faster, and it takes 8 MB in place of 12 MB or more.

Single-threaded, which is the wasm case, the first three items took `balanced`
from 207 ms to about 130 ms. The bucket sort and the index also help in that
case, and this pass left their sequential figures unmeasured.

Verification: the `record-groove` unit tests pass, clippy passes, the
`wasm32-unknown-unknown` check passes for `record-groove` and `record-cut-wasm`,
and the full `record-render`, `record-decode`, `record-cut` and `test-spin`
suites pass. Two failures predate this change: the README doctests, and two
`--ignored` tests that read a `single45` fixture that is absent from disk.

`record-groove/examples/tone_bench.rs` is the benchmark. Run
`cargo run --release -p record-groove --example tone_bench -- 1.2 1.15 1.1`. It
prints the timing, the chosen config, the mean colour, the drift and a palette
hash for each tone.

## Carrier overhead

The overhead is `24 / bits_per_pixel − 1`. `GROOVE_TONE_MAX_SIZE_FACTOR = 1.2`
in `record-render/src/lib.rs` holds it at 20 bpp. Fewer pixels need more bits
per pixel, and more bits per pixel need more colours near the tone. The gamut
holds a fixed number of such colours, so the palette drifts further from the
tone. Measured on the same house tone:

| budget | bpp | overhead | mean colour | max drift |
|---|---|---|---|---|
| 1.2 (now) | 20 | 20% | `CBCEBF` | 164 |
| 1.15 | 21 | 14% | `B9BCAC` | 227 |
| 1.1 | 22 | 9% | `A5A998` | 296 |

21 bpp recovers 6% of the surface, and it gives a greyer and grainier record.
22 bpp appears as pastel static, which the `record-groove` README calls "warm
khaki". Palette memory and decode time each about double per extra bit. A change
to the constant is compatible with the format, because the tone-span descriptor
carries the bits per pixel, and older records therefore still decode. The
constant sets the appearance of a record, and this pass left it at 1.2.

One change reduces the overhead below a full extra bit:

1. **Non-power-of-two palettes.** Pack pixel pairs, so a palette of about 1.5 M
   colours carries 41 bits per two pixels, which is 20.5 bpp at 17% overhead.
   This packing gives a finer trade-off ladder, and it needs a new encoding
   version.

The groove alone carries the tone. The BRS1 prefix is untoned already, so it
offers no further saving.
