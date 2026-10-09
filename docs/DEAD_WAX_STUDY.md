# Dead wax study

This document compares the lead-out that bitneedle cuts at lead-out geometry
revision 3 with a lead-out that follows IEC 60098 and RIAA practice. The
example `record-render/examples/dead_wax_study.rs` renders both.

## Terms

- **Programme**: the groove that carries the stream.
- **Silent groove**: the fine groove after the programme, at 2 px from turn
  to turn.
- **Lead-out**: the wide spiral after the silent groove.
- **Locked groove**: the closed ring at the end of the lead-out. IEC 60098
  calls it the finishing groove.
- **Land**: the vinyl between the locked groove and the label. A pressing
  plant stamps or etches its marks into the land and between the lead-out
  turns.
- **Dead wax**: the lead-out, the locked groove and the land.

## Reference dimensions

| Item | 12" and 10" | 7" | Source |
|---|---|---|---|
| Locked groove diameter | 106.4 ± 0.8 mm | 97.4 ± 1.0 mm | IEC 60098 (1987) |
| Minimum inside diameter of the programme | 120.65 mm (4¾") | 108 mm (4¼") | RIAA, IEC 60098 (1958) |
| Lead-out pitch | 6.4 ± 3.2 mm a turn, at least one turn | same | IEC 60098 (1987) |
| Lead-out pitch | 2 to 6 grooves an inch | 2 to 5 grooves an inch | RIAA |

The minimum inside radius of each profile (`payload_inner_radius`) agrees
with these figures: 115 px on `lp`, 138 px on `ten` and 177 px on
`single45`. The locked-groove radii in `README.md` (101.7, 122.0 and 162.5 px)
also agree with them.

## Revision 3

- The locked groove is 1 mm outside the label (`LOCK_GROOVE_CLEARANCE_MM`).
- The programme may cut down to the lock, two run-out turns at 2.5 px and
  two silent-groove turns (`programme_floor_radius`). This floor is under the
  minimum inside radius on each profile.
- The run-out fills the room that the cut leaves (`LeadOutExtent::Fill`).
  Its rings are 5 to 8 px apart on a short side and close to 2 px on a full
  side.

`README.md` and the comment on `RUN_OUT_TURNS` state that a pressed record
keeps two lead-out turns and a smooth land for matrix and stamper marks.
Revision 3 does not keep the land.

## Proposal

- The locked groove is at the finishing-groove radius of IEC 60098.
- The lead-out has at least two turns at one constant pitch. The pitch is
  6.4 mm a turn when the room permits it, and it is never less than 3.2 mm.
- The land from the locked groove to the label stays bare.
- The programme does not cut below the larger of the minimum inside radius
  and the radius that holds the lead-out and two silent-groove turns.

The **etched** variant also keeps the lead-out turn next to the locked groove
at 6.4 mm, so each record has one 6.4 mm band for marks.

## Results

The renders use the `lori-asha-westside-lp-hq` stream at 60,000 bytes for a
short side. A full side is the largest stream that each layout holds.

| Profile | Layout | Full side (bytes) | Of revision 3 | Lead-out gaps, full side | Land | Trailer, full side (bytes) |
|---|---|---:|---:|---|---|---:|
| `single45` | Revision 3 | 426,054 | 100% | 0.6 mm | 1.1 mm | — |
| `single45` | Proposal | 356,444 | 83.7% | 2 turns at 3.6 mm | 2.8 mm | 2,172 |
| `single45` | Etched | 323,573 | 75.9% | 3.9 mm, then 6.4 mm | 2.8 mm | 2,190 |
| `ten` | Revision 3 | 513,066 | 100% | 0.9 mm | 1.1 mm | — |
| `ten` | Proposal | 476,327 | 92.8% | 2 turns at 3.8 mm | 3.4 mm | 1,598 |
| `ten` | Etched | 458,925 | 89.4% | 4.2 mm, then 6.4 mm | 3.4 mm | 1,598 |
| `lp` | Revision 3 | 561,406 | 100% | none (1 ring) | 1.3 mm | — |
| `lp` | Proposal | 534,335 | 95.2% | 2 turns at 3.9 mm | 3.3 mm | 1,325 |
| `lp` | Etched | 522,734 | 93.1% | 4.5 mm, then 6.4 mm | 3.3 mm | 1,335 |

On a short side, the proposal cuts 4 to 11 lead-out turns at 6.0 to 6.7 mm,
where revision 3 cuts 13 to 19 rings at 1.5 to 4.2 mm.

The trailer is the lead-out and the locked groove at six bits a pixel. Each
layout holds more than the 512 bytes that the format guarantees.

## Implementation

The proposal changes the geometry that a reader derives, so it is a new
revision of `SEGMENT_LEAD_OUT_GEOMETRY` (35): revision 4.

1. `record-core`: a lead-out geometry for each revision. Revision 4 sets the
   lock radius from the IEC table, a constant-pitch lead-out of at least two
   turns, and the programme floor above. `lock_radius_px`,
   `programme_floor_radius`, `lead_out_geometry_with_extent`,
   `build_lead_out_trace`, `build_silent_groove_spiral_indices` and
   `build_run_out_spiral_indices` take the revision.
2. `record-core`: `cut_inner_radius_from_geometry` takes the floor when the
   floor is above `payload_inner_radius`. On `single45` the revision 4 floor
   is above it.
3. `record-descriptor`: `LEAD_OUT_GEOMETRY_REVISION_PRESSED = 4`.
4. `record-cut` writes the revision that the render used.
5. `record-decode`, `record-sidecar` and `record-wasm` trace the trailer and
   the silent groove at the revision that the descriptor names. A revision 3
   record then still reads its trailer.
6. `bitneedle-app`: the capacities in `programme-fit.mjs` (`capacityBytes`)
   change by the factors in the table.

## Decisions for the owner

- The proposal or the etched variant.
- Whether revision 4 is the default for each new cut, or a render option.
- Whether a short side cuts many lead-out turns at 6.4 mm, or a few lead-out
  turns below a wider bare band.

## Study branch

The branch `dead-wax-study` lets `STUDY_PROGRAMME_FLOOR_<PROFILE>` set the
programme floor, so the renderer cuts a full side under the proposed floor.
This override is for the study and is not for `main`.
