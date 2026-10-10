# Dead wax study

This document gives the measurements of the dead wax that lead-out geometry
revision 4 cuts, and compares them with revision 3. The example
`record-render/examples/dead_wax_study.rs` makes the measurements.

## Terms

- **Programme**: the groove that carries the stream.
- **Silent groove**: the fine groove after the programme, at 2 px from turn
  to turn.
- **Run-out**: the wide spiral after the silent groove. IEC 60098 calls it
  the lead-out groove.
- **Locked groove**: the closed ring at the end of the run-out. IEC 60098
  calls it the finishing groove.
- **Land**: the vinyl between the locked groove and the label. A pressing
  plant stamps or etches its marks into the land and between the run-out
  turns.
- **Dead wax**: the run-out, the locked groove and the land.

## Reference dimensions

| Item | 12" and 10" | 7" | Source |
|---|---|---|---|
| Locked groove diameter | 106.4 ± 0.8 mm | 97.4 ± 1.0 mm | IEC 60098 (1987) |
| Minimum inside diameter of the programme | 120.65 mm (4¾") | 108 mm (4¼") | RIAA, IEC 60098 (1958) |
| Lead-out pitch | 6.4 ± 3.2 mm a turn, at least one turn | same | IEC 60098 (1987) |
| Lead-out pitch | 2 to 6 grooves an inch | 2 to 5 grooves an inch | RIAA |

The minimum inside radius of each profile (`payload_inner_radius`) agrees
with these figures: 115 px on `lp`, 138 px on `ten` and 177 px on
`single45`.

## Revision 3

- The locked groove is 1 mm outside the label (`LOCK_GROOVE_CLEARANCE_MM`).
- The programme can cut down to the lock, two run-out turns at 2.5 px and
  two silent-groove turns. This floor is below the minimum inside radius on
  each profile.
- The run-out fills the room that the cut leaves. Its rings are 5 to 8 px
  apart on a short side and approximately 2 px apart on a full side.

## Revision 4

- The locked groove is at the lower limit of the finishing-groove diameter
  of IEC 60098: 96.4 mm on a 7", and 105.6 mm on a 10" or a 12".
- The land from the locked groove to the label is bare.
- The run-out has at least one turn at one constant pitch. The pitch is
  6.4 mm a turn when the room permits it. The pitch is a minimum of 3.2 mm.
- The programme floor is the radius that holds one run-out turn at 3.2 mm,
  two silent-groove turns and one raster gap above the locked groove. A full
  side cuts inside the minimum inside diameter of recording.

The encoder writes revision 4 in `SEGMENT_LEAD_OUT_GEOMETRY` (35). A reader
traces the trailer, the silent groove and the programme mask at the revision
that the descriptor names. A record that has revision 3 or an earlier
revision reads with the revision 3 geometry. A sidecar rewrite keeps the
revision of the record (`RecordDescriptorInput::lead_out_geometry_revision`).

## Reasons for the layout

The dead wax of a full side is as narrow as the playback rules of IEC 60098
permit.

The locked groove and the run-out keep to IEC 60098, because a turntable
uses them:

- An automatic turntable starts its return when the arm reaches the
  finishing-groove diameter, or when the arm moves inward faster than the
  programme feed. The run-out pitch of 6.4 ± 3.2 mm gives that fast movement.
  Thus each side keeps at least one run-out turn of 3.2 mm or more.
- The locked groove stops the stylus. Its diameter has a tolerance, and the
  label is fixed, so the land is the remaining vinyl. The lock is at the
  lower limit of the tolerance, so the land is as narrow as the standard
  permits: 2.3 mm on a 7", 3.0 mm on a 10" and 2.9 mm on a 12".
- A pressing plant etches or stamps its matrix marks into the land.

The minimum inside diameter of recording (RIAA, and IEC 60098 of 1958) does
not apply to the programme:

- The rule limits inner-groove distortion. Near the centre, the groove speed
  is low and a stylus cannot trace short wavelengths accurately.
- The bitneedle programme is a picture of data, not an audio groove, so the
  reason for the rule does not apply.
- A full side cuts down to the run-out. This removes 0.6 mm (7"),
  1.7 mm (10") and 1.3 mm (12") of dead wax, and adds capacity.

The two-turn layout and a layout with no run-out turn were also measured.
The two-turn layout costs capacity and does not help a turntable. A layout
with no run-out turn breaks the run-out rule of IEC 60098.

## Results

The renders use the `lori-asha-westside-lp-hq` payload. A short side is
60,000 bytes. A full side is the largest payload that the profile holds, to
the nearest 1,000 bytes. The revision 3 figures come from the same example
at f4e03db.

| Profile | Full side, revision 3 (bytes) | Full side, revision 4 (bytes) | Of revision 3 |
|---|---:|---:|---:|
| `single45` | 426,054 | 396,083 | 93.0% |
| `ten` | 513,066 | 494,696 | 96.4% |
| `lp` | 561,406 | 548,837 | 97.8% |

| Profile | Side | Programme stops at | Run-out | Locked groove | Land | Trailer (bytes) |
|---|---|---|---|---|---|---:|
| `single45` | Short | 243 px | 4 turns at 6.0 mm | 96.4 mm | 2.3 mm | 3,996 |
| `single45` | Full | 175 px | 1 turn at 3.2 mm | 96.4 mm | 2.3 mm | 1,301 |
| `ten` | Short | 242 px | 8 turns at 6.3 mm | 105.6 mm | 3.0 mm | 6,537 |
| `ten` | Full | 135 px | 1 turn at 3.6 mm | 105.6 mm | 3.0 mm | 978 |
| `lp` | Short | 244 px | 11 turns at 6.6 mm | 105.6 mm | 2.9 mm | 8,297 |
| `lp` | Full | 113 px | 1 turn at 3.4 mm | 105.6 mm | 2.9 mm | 797 |

The trailer is the run-out and the locked groove at six bits a pixel. Each
cut holds more than the 512 bytes that the format guarantees.

A short side keeps the programme at its nominal pitch, and the run-out takes
the remaining room in turns of approximately 6.4 mm. A full side compresses
the programme pitch, and the run-out has one turn.

## Two-turn layout

The study also measured a layout with a minimum of two run-out turns at
3.2 mm. Its full sides held 83.7% (`single45`), 92.8% (`ten`) and 95.2%
(`lp`) of revision 3. Revision 4 uses one turn, because IEC 60098 asks for
one turn, and the land inside the locked groove holds the marks of the plant.
