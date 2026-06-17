# bitneedle

Bitneedle is the public reference code for reading and validating Bitneedle
picture-record objects: visual record-like images that carry recoverable audio
data.

This repository is published so artists, collectors, archivists, marketplaces,
and independent developers can inspect Bitneedle objects, verify that the audio
is carried by the object itself, and build self-sufficient playback and
preservation tools.

## Open decoder, reserved creation rights

The source code in this repository is licensed under the Apache License,
Version 2.0, in `LICENSE`.

Certain Bitneedle picture-record encoding, authoring, record-creation,
commercialization, and marketing technologies are patent pending. Publication
of this repository is not intended to dedicate those technologies to the public,
waive patent rights, or grant rights to make, market, sell, mint, inscribe,
issue, or commercially provide Bitneedle records or Bitneedle-compatible
creation tools.

Wavey intends the public Bitneedle format materials and decoder code to support
interoperability, verification, playback, archiving, research, and user
self-custody. See `PATENTS.md` for the patent notice and limited
interoperability pledge.

## Repository scope

- `record-decode`: decode and inspect Bitneedle picture-record images.
- `record-core`: shared geometry and record-profile primitives used by the
  decoder.
- `record-descriptor`: metadata descriptor parsing and serialization.
- `record-sidecar`: sidecar structures used to support recovery and inspection.
- `bytes2rgb`: pixel-to-byte utilities used by decoder and verification tools.

This repository is not the commercial Bitneedle record authoring platform and
does not grant a license to use Bitneedle patents for commercial encoding,
creation, issuance, minting, inscription, branding, or marketing of records.
