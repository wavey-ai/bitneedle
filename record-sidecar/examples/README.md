# record-sidecar examples

Runnable spikes against the Sidecar crate. Build any of them with
`cargo run --example <name> -- <args>`; each prints its usage line when
run bare.

## The inscription spikes (Aug 2026) — the signature that carries the image

Three spikes exploring one idea, kept deliberately: **the fan's
inscription — the signed and numbered handwriting on an edition's label —
can itself be the Sidecar carrier.** Not ink next to data: ink that *is*
data. This is a candidate treatment for future edition types; it is NOT
what production editions ship today (see "what production uses" below).

The progression:

1. **`inscription_label_spike`** — the control case. A stamped record and
   an AVIF pushed through the standard, long-standing library path:
   `rewrite_record_png` with `carriers: ["label"]`. Verifies the BRS1
   audio payload is untouched and the image round-trips. Proof that the
   ordinary API already does "record + image in, authored record out"
   with no custom work.

2. **`inscription_only_spike`** — restricts the carriers to the
   inscription's own glyph pixels (supplied as a mask). The handwriting
   alone holds the hidden image. Capacity is small (glyph pixels only),
   and the report notes the catch: *a decoder can only rebuild this
   carrier set if the inscription geometry ships in a future record
   descriptor version.* That geometry-in-descriptor step is the price of
   admission for this treatment.

3. **`pairformed_inscription_spike`** — the full concept. Each carrier
   pair's **common mode forms the visible pencil relief** while the
   **pair differential carries the sign and magnitude bits**: the
   signature's shading is literally sculpted by the same pixel writes
   that store the payload. Ink and data are one operation. Same
   descriptor-geometry requirement as (2).

### What production uses

Shipping editions embed their secret image with the standard months-old
API — sign and number the label visually, composite it into the record,
then `rewrite_record_png(record, { sidecar: { carriers: ["label"],
items: [image] } })` and verify with `decode_record_png_sidecar_bytes`.
Full label-region capacity, decodable by every existing record decoder,
zero bespoke geometry.

When a future edition type wants the inscription-as-carrier treatment,
start from spike (3), and budget for the descriptor version bump that
records the inscription geometry.

## Other examples

- **`patch_cache_encryption`** — migrates a record PNG's cache-encryption
  secret in place (see `patch_record_png_cache_encryption_secret`).
