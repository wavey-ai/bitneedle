# record-proof

Print-proof post-processing for Bitneedle picture records.

`record-proof` reads a rendered 576×576 record PNG and writes a second PNG. The
second PNG keeps the disc byte-for-byte and keeps the background transparent. It
adds color-calibration targets in the four corners. A scanner reads these
targets to correct the color rendition of the printer, and a decoder then reads
the printed record.

```
cargo run -p record-proof -- path/to/name.record.png            # writes name.proof.png
cargo run -p record-proof -- in.png out.png --json               # prints the layout config
```

Layout `proof-v1`:

- **Top-left** – a QR code (binary, EC level M). It carries the layout
  parameters and every distinct toned-groove palette config: base tone, luma
  tolerance, bits per pixel, ordering and byte length. A scanner rebuilds the
  exact expected swatch colors from this code.
- **Other three corners** – identical swatch grids, mirrored to anchor at
  their own corner. Each grid holds a registration marker, then
  black/white/grays/RGBCMY, then every palette color of every tone span in
  palette-index order. A record with plain grooves carries a 4-level RGB cube
  in place of the palette colors.

Every painted pixel is a deterministic function of the record descriptor.
`ProofLayout::for_descriptor` therefore regenerates the expected color of every
pixel block on the decode side.
