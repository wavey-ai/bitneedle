# record-sidecar examples

Runnable spikes against the Sidecar crate. Build any of them with
`cargo run --example <name> -- <args>`. Each example prints its usage line when
you run it with no arguments.

## Inscription spikes (August 2026)

Three spikes examine one idea: **the inscription of a fan — the signed and
numbered handwriting on the label of an edition — carries the Sidecar payload
itself.** The written marks hold the data. This treatment is a candidate for
future edition types. Production editions use the treatment in "Production
treatment" below.

The three spikes progress as follows:

1. **`inscription_label_spike`** — the control case. It pushes a stamped record
   and an AVIF through the standard library path, `rewrite_record_png` with
   `carriers: ["label"]`. It verifies that the BRS1 audio payload holds its
   bytes and that the image round-trips. It shows that the ordinary API takes a
   record and an image and returns an authored record.

2. **`inscription_only_spike`** — it restricts the carriers to the glyph pixels
   of the inscription, supplied as a mask. The handwriting alone holds the
   hidden image. Capacity is small, because the glyph pixels are few. The report
   states one condition: *a decoder rebuilds this carrier set when the
   inscription geometry ships in a future record descriptor version.* This
   treatment therefore needs that descriptor change.

3. **`pairformed_inscription_spike`** — the full concept. The **common mode** of
   each carrier pair forms the visible pencil relief. The **pair differential**
   carries the sign bits and the magnitude bits. The same pixel writes therefore
   shade the signature and store the payload. This spike needs the same
   descriptor geometry as spike 2.

### Production treatment

A shipping edition embeds its secret image through the standard API. Sign and
number the label visually, composite it into the record, then run
`rewrite_record_png(record, { sidecar: { carriers: ["label", "intergroove"],
items: [image] } })`. Verify the result with
`decode_record_png_sidecar_bytes`. Both carriers are used: the label holds about
8.5 KB on a 576 px single, and the intergroove takes the same image to about
27 KB. Every existing record decoder reads this treatment, and it needs no
bespoke geometry.

Read the payload back with `decode_record_png_sidecar_bytes(png, profile)`. The
carrier set, the seed and the scheme come from the BSC1 descriptor pointer of
the record, so one call reads a label-only pressing and a label plus intergroove
pressing. A hand-written annulus walk reads the label carrier alone, and it
misses the intergroove half of a modern edition.

For a future edition type that uses the inscription as the carrier, start from
spike 3, and plan for the descriptor version bump that records the inscription
geometry.

## Other examples

- **`patch_cache_encryption`** — migrates the cache-encryption secret of a
  record PNG in place. See `patch_record_png_cache_encryption_secret`.
