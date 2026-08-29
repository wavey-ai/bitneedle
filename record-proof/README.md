# record-proof

Print-proof post-processing for Bitneedle picture records.

`record-proof` takes a rendered 576×576 record PNG and writes a second PNG
with the disc byte-for-byte untouched, the background still transparent, and
colour-calibration targets painted into the four corners so a high-quality
print can later be scanned and decoded despite the printer's colour
rendition.

```
cargo run -p record-proof -- path/to/name.record.png            # writes name.proof.png
cargo run -p record-proof -- in.png out.png --json               # prints the layout config
```

Layout `proof-v1`:

- **Top-left** – QR code (binary, EC level M) carrying the layout parameters
  and every distinct toned-groove palette config (base tone, luma tolerance,
  bits per pixel, ordering, byte length). A scanner can rebuild the exact
  expected swatch colours from this alone.
- **Other three corners** – identical swatch grids, mirrored to anchor at
  their own corner: a registration marker, then black/white/greys/RGBCMY,
  then every palette colour of every tone span in palette-index order.
  Records without toned grooves get a 4-level RGB cube instead.

Everything painted is a deterministic function of the record descriptor, so
`ProofLayout::for_descriptor` regenerates the expected colour of every pixel
block on the decode side.
