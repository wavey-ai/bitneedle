# Toned palette: auto-tune speed and the 20% overhead

Notes from a pass over `record-groove` on 2026-09-04, prompted by the
question: can the auto-tone be made faster, and can the extra 20% of carrier
it costs be reduced? Yes on the speed. The 20% is a different story: it is a
colour-quality knob, not an inefficiency.

## Speed (done, bit-identical)

The toning cost is almost entirely `TonedConfig::balanced` (the auto-tune)
plus building the palette twice on the cut path (track tone + gap tone).
Encoding the pixels is under 1 ms. Measured on the house tone `#F2EEE5`,
release build, an 8-core machine:

| step | before | after |
|---|---|---|
| `balanced` (auto-tune) | 207 ms | 41 ms |
| palette build (×2 on cut) | 68 ms | 21 ms |
| decode (140 KB payload) | 40 ms | 20 ms |
| **cut path total** | **~340 ms** | **~85 ms** |

What changed, all producing the same palettes (15 configs across 5 tones × 3
budgets were fingerprinted before and after — identical hashes):

- **Blue range by bisection** instead of testing every blue value — rounded
  luma is monotone in blue, so the window's ends are found in 8 probes on the
  same predicate. This alone makes the ladder's `iso_luma_count` calls (9 of
  them) nearly free.
- **Hoisted the base tone's chroma** and passed each colour's luma through
  instead of recomputing it three times per colour.
- **One tally per colour** in the ladder histogram (under its narrowest rung,
  prefix-summed after) instead of one per rung.
- **Multi-threaded enumeration** on native (`std::thread::scope`, no new
  deps; sequential on wasm32 where there are no threads). Folds are sums or
  sorted collections, so thread order can't change the result.
- **Palette sort by bucket runs** in parallel instead of `select_nth` + one
  1M-element sort.
- **Reverse index as a sorted packed `Vec<u64>`** instead of a SipHash
  `HashMap` — faster to build, 8 MB instead of ~12+.

Single-threaded (i.e. what wasm gets), `balanced` went 207 → ~130 ms from the
first three items; the bucket sort and index help there too but sequential
numbers for those were not measured.

Verified: `record-groove` unit tests, clippy, `wasm32-unknown-unknown` check
for `record-groove` and `record-cut-wasm`, and the full
`record-render`/`record-decode`/`record-cut`/`test-spin` suites all pass. Two
pre-existing failures are unrelated: the README doctests (already broken
before this change) and two `--ignored` tests that read a `single45` fixture
that isn't on disk.

`record-groove/examples/tone_bench.rs` is the benchmark —
`cargo run --release -p record-groove --example tone_bench -- 1.2 1.15 1.1`
prints timing, chosen config, mean colour, drift and a palette hash per tone.

## The 20%

The overhead is exactly `24 / bits_per_pixel − 1`, and
`GROOVE_TONE_MAX_SIZE_FACTOR = 1.2` in `record-render/src/lib.rs` pins it to
20 bpp. Fewer pixels means more bits per pixel, which means the palette needs
more colours near the tone — and there aren't more, so it drifts further. Same
house tone, measured:

| budget | bpp | overhead | mean colour | max drift |
|---|---|---|---|---|
| 1.2 (now) | 20 | 20% | `CBCEBF` | 164 |
| 1.15 | 21 | 14% | `B9BCAC` | 227 |
| 1.1 | 22 | 9% | `A5A998` | 296 |

So 21 bpp buys 6% of surface for a visibly greyer, grainier record; 22 bpp
reads as pastel static (the `record-groove` README's "warm khaki"). Also note
palette memory and decode time roughly double per extra bit. Changing the
constant is safe format-wise — bpp travels in the tone-span descriptor, so old
records still decode — but it's a look decision, so it has not been touched.

Two things that would actually shave overhead without going to a full extra
bit:

1. **Non-power-of-two palettes** — pack pixel pairs, so a palette of ~1.5 M
   colours carries 41 bits per two pixels (20.5 bpp, 17% overhead). Finer
   trade-off ladder, but it's a new encoding version.
2. **Tone only the groove, not the BRS1 prefix** — already the case; nothing
   to gain there.
