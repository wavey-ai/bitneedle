# Lead-out, groove shape, and the programme fit — handover

Session state for the work on the lead-out geometry, the groove rasterisation,
and the programme fit in `record-core` / `record-render`. Written to restart
without re-deriving it. Everything below is on `main` in `bitneedle`; no
`goldenfiles/` were added to on purpose — the sweep writes to `/tmp`.

## Policy (what we are aiming for)

The cut should read like a real record: a realistic programme pitch, a fine
silent groove band, a short run-out into a lock, and the artwork still visible.

- **Programme pitch** floor **1 px/turn** (turns may abut), ceiling **2.7
  px/turn**. Short programmes sit at the ceiling and are not spread wider; a
  real side would not optimise the fit by approaching the label at a wide pitch
  — audio sounds better and louder away from the label. The leftover goes to
  the run-out and silent groove, not to a wider programme.
- **Fit the pitch to the band.** Reserve the run-out and a few silent groove turns,
  derive `b` for the programme to just reach that reserve, clamp to `[1, 2.7]`,
  then solve the depth exactly at that pitch. Never let the pitch slip below the
  floor to make the arithmetic work; fail instead if the side cannot hold it.
- **Silent groove**: 2 px centre-to-centre (one pixel groove + one pixel daylight —
  "1 px gap"). Always keep a few turns: **2 minimum** (sidecar capacity), and
  **≤3** when the programme runs dense. Once the programme is under **2 px/turn**
  the silent groove becomes a sliver and the run-out only a fraction of a turn (≈¼)
  before the lock; the programme takes the room. At **≤1.5 px/turn**, silent groove is
  capped at 3 turns.
- **Run-out**: full when the programme is short, collapsing to a quarter turn
  (and the lock, one closed ring) when the programme is dense.
- **Anti-moiré**: vary each record's `b` by **seeding the run-out gap**, not `b`
  itself. The gap sets the reserve, the reserve sets the feasible `b`, and the
  depth search lands it, so pitch and moiré vary while the fit stays exact.
  Jitter the silent groove pitch on the same seeded phase; seed from the release id so
  a re-press is identical (the reader reads `b` from the prefix and never needs
  the seed).

## What this session did

### 1. One walk, one hand

- `record-core/src/lib.rs::trace_groove_into` is now the single spiral walk.
  `trace_record_spiral_with_family` is a thin wrapper. It carries
  `theta`/`theta_effective`, returns `(angle, radius)`, and takes a `GrooveStep`
  profile closure so the programme (Archimedean / vari-pitch), the silent groove, and
  the run-out all step identically.
- **`join_groove_chain`** runs *inside* the trace (and after the lock loop). It
  removes a staircase pixel only when the pixels on both sides of it already
  touch, so the groove is one pixel wide with a one-pixel gap and never broken.
  This is not a mode: "thin" is what a groove is. `thin_chain`,
  `completes_dense_two_by_two`, and the experimental flag are gone. The
  record-render duplicate tracer and the duplicate in `count_spiral_mask_pixels`
  were deleted; a divergence there was the original cause of payloads that did
  not round-trip.
- **Handedness flipped to the lathe hand (anticlockwise inward).**
  `trace_groove_into` uses `angle = start_angle + if clockwise { -theta } else
  { theta }`. House defaults flipped: `build_band_spiral_indices` /
  `..._at_angle` pass `false`; `build_spiral_mask_with_family` passes `false`;
  `record-descriptor`'s `spiral_clockwise` defaults `false`; `record-cut`'s
  `RecordDescriptorInput` field was renamed `spiral_anticlockwise` →
  `spiral_clockwise` (segment 34 writes `[1]` only for a clockwise cut);
  `groove_angle_at_radius` takes `clockwise` (it was still winding the old way
  and putting the silent groove origin on the wrong side of the disc).

### 2. Lead-out geometry

- `trace_lead_out` in `record-core` traces **silent groove → run-out → lock as one
  continuous groove** with one `occupied`, carrying angle across the joins.
  The lock is an integer-pixel ring (`round(2π·lock_radius)`) that closes on
  itself; the old `while locked < 2π` left a seam.
- Option (a): the run-out entry is a consequence of the silent groove.
  `silent_groove_inner_radius = max(entry, cut - SILENT_GROOVE_MAX_TURNS·sep)`.
- `LOCK_MERGE_PX = 1.0` (descent stops one pixel above the lock so the rows
  touch), `LOCK_GROOVE_CLEARANCE_MM = 1.0` (lock at `label + 1 mm`).
- `RUN_OUT_RING_FACTOR = 2.0` doubles the run-out rings.
- **`silent_groove_turn_separation_px` is now a flat `2.0` px** — one pixel of groove
  and one of daylight. (This was the "solid thick grey ring": it had been set to
  `1.0`, which shares pixels and merges the turns.)
- Fill silent groove tiers, partly wired: `FILL_SILENT_GROOVE_MIN_TURNS = 2`,
  `FILL_SILENT_GROOVE_MAX_TURNS = 3`, `FILL_RUN_OUT_MIN_TURNS = 0.25`.

### 3. The programme fit (rewritten)

`record-render/src/lib.rs::solve_cut` no longer fits a *span*:

1. reserve the run-out + `SILENT_GROOVE_RESERVE_TURNS = 2` and
   `RUN_OUT_RESERVE_TURNS = 0.25`;
2. derive `b = (R_out² − R_reserve²) / 2N`;
3. clamp `2πb` hard to `[MIN_PROGRAMME_TURN_SEPARATION_PX = 1,
   MAX_PROGRAMME_TURN_SEPARATION_PX = 2.7]` (these live in `record-core`);
4. if the whole band at that pitch cannot hold `N`, go finer to the coarsest
   pitch that can; error at the floor (`fit_span_at_pitch`'s message);
5. `fit_span_at_pitch` holds `b` fixed and binary-searches the **depth**, so the
   pitch can never slip below the floor to make the arithmetic come out.

### 4. Reference label

`RenderOptions.label_reference` (JSON `labelReference`) paints the label disc
*and the profile's own centre features* (`paint_label_reference`): paper, the
45's dink, its knockout, the spindle hole. Without it every label looked like an
LP's.

### 5. Sweep harness (in `/tmp`, not goldens)

`record-render/examples/record_sweep.rs`:

```
cargo run -p record-render --example record_sweep -- <profile> <outDir> [sample.ecdc]
```

- Programmes are the **real frames** of the sample: one ECDC packet = 1.33 s, so
  durations are counted, not guessed.
- Sample used: `../yl.vin/.tmp-local/preload-pray4me/pray4me-confirmation.12kbps.1333ms.ecdc`
  (167 frames · 1697 B/frame · 3.71 min; **nc=8**, so not the 7-codebook cut).
- Writes PNGs + `index.html` (black page, the wheel-lab `web/img/sample.avif`
  under every groove) to `outDir`; `open <outDir>/index.html`.
- Last run: `single45` reaches 5:00 at 1.04 px/turn; `lp` reaches 5:00 at 1.07.

## Resolved this session

The lead-out is now one reserve-driven band, the same on every profile.

- **Naming.** The fine filler between the programme and the run-out is the
  **silent groove**; the **deadwax** is the run-out plus the locked groove.
  Renamed across the workspace (`silent_groove_*`, `SILENT_GROOVE_*`,
  `SilentGroove`), including the descriptor field and the sidecar carrier.
- **Silent groove is flat 2 px** centre-to-centre on every profile and at every
  programme pitch — one pixel of groove, one of daylight. `SILENT_GROOVE_PITCH_MM`
  is retired; the band is no longer cut at a physical feed. Two turns, three
  when the cut stopped early (`FILL_SILENT_GROOVE_MIN/MAX_TURNS`).
- **Run-out is elastic and pixel-based.** `RUN_OUT_TURN_SEPARATION_PX = 16`,
  `RUN_OUT_INNER_GAP_PX = 10`, tapering outward, and it may close to
  `RUN_OUT_MIN_TURN_SEPARATION_PX = 2.5` on a full side. It never closes below
  `RUN_OUT_MIN_TURNS = 2`, plus the one lock ring. Stated in pixels because the
  mm feed drew the 12" rings at ~60% of the 7" spacing — a fat band.
- **Programme floor.** `programme_floor_radius = lock + 2 run-out turns + 2
  silent-groove turns`, from the profile alone. `solve_cut` reserves it, derives
  `b = (R_out² − R_floor²)/2N`, clamps `2πb` to `[1, 2.7]`, then solves the
  depth exactly at that pitch (`fit_span_at_pitch`). `programme_inner_radius`
  and `cut_inner_radius_from_geometry` now use the floor, and
  `build_spiral_mask_with_handedness` (reader) uses the same inner cut as the
  renderer's mask — they disagreed before, which broke round-trip.
- **Pitch variation, not anti-moiré.** A programme held at the 2.7 ceiling is
  cut at `ceiling × factor`, `factor ∈ [PITCH_VARIATION_MIN_FACTOR, 1)`, seeded
  from the release id (FNV-1a) so a re-press is identical. It is **one scalar
  per record**, not a dither: the spiral stays uniform, so the artwork stays as
  visible as it was. Programmes that reach the reserve are not walked. The
  reader reads `b` back, so nothing else changes.
- **Lead-in continuity.** `band_window` trims the traced band to a contiguous
  slice instead of testing each rounded radius, which had punctured the outer
  turn into dashes.
- **Sweep page.** `record_sweep` now takes `SWEEP_PROFILES` (comma list, default
  all four) and writes one page with a section per profile; every row carries a
  valid `rel_…` release id so the pitch variation is visible.

## Test status

- `record-core` 126, `record-descriptor` 39, `record-cut` 39, `record-decode`
  20, `record-sidecar` 4, `record-wasm` 2, `test-spin` 3+6: all pass.
- `record-render`: 39 pass, 2 ignored. Goldens re-blessed with
  `cargo test -p record-render --lib regenerate_golden_records -- --ignored`.
- `cargo check --workspace --all-targets` clean apart from pre-existing
  `VARI_PITCH_*` / `sort_slices_by` / `build_sidecar_carrier_pairs` warnings.

## Open

- The formal `bitneedle-format/*.txt` drafts still say "deadwax" for the fine
  band (draft 00 also uses it for the label-clearance region, where it is
  correct). Left untouched on purpose; a spec pass should settle the wording.

## Files touched

- `record-core/src/lib.rs`: `MIN/MAX_PROGRAMME_TURN_SEPARATION_PX`,
  `RUN_OUT_RING_FACTOR`, `LOCK_MERGE_PX`, `LOCK_GROOVE_CLEARANCE_MM`,
  `FILL_*`, `trace_groove_into`, `join_groove_chain`, `trace_lead_out`,
  `build_lead_out_trace`, `build_silent_groove_spiral_indices`,
  `build_run_out_spiral_indices`, `lead_out_geometry_with_extent`,
  `silent_groove_turn_separation_px`, `groove_angle_at_radius`,
  `build_spiral_mask_with_handedness`, `programme_inner_radius`.
- `record-render/src/lib.rs`: `solve_cut`, `fit_span_at_pitch`,
  `evaluate_spiral_fit`, `count_spiral_mask_pixels`, `build_spiral_mask`,
  `paint_label_reference`, `RenderOptions.label_reference`.
- `record-render/examples/record_sweep.rs` (new).
- `record-descriptor/src/lib.rs`, `record-cut/src/descriptor/encode.rs`
  (handedness).
- `record-decode/src/lib.rs`, `record-sidecar/src/lib.rs`,
  `record-wasm/src/lib.rs` (handedness threading).

## Also done this session (different task, same tree)

`bitneedle-app/native/.../record_decode_bridge.rs`: the BRS1 decode rebuilt one
ECDC object per descriptor *run* with a stale per-chunk `al`, so
`lmEcdcDecodeChunks` threw "111 chunks, but metadata implies 1". It now wraps
**each payload entry in its own standalone ECDC header** with `al =
output_samples`. `record-core::ecdc::payload_to_standalone_ecdc` was changed to
derive `al` from the descriptor's `output_samples`. Verified against
`~/Downloads/yl_EPAGQWD5YFX.png`.
