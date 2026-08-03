# BPK1 Bitneedle Package

BPK1 is an optional binary container for exact Bitneedle record components.

A package contains one BRD1 descriptor and one BRS1 stream. It can also contain one BSC1 sidecar.

BPK1 transports exact BRD1, BRS1, and BSC1 bytes before PNG rendering.

The encoder validates all components. It also validates cross-component lengths, hashes, and sidecar pointers.

```text
BPK1
├── BRD1
├── BRS1
└── BSC1 (optional)
```

Use `encode_package` to create a package. Use `parse_package` to validate and access its sections.

See `../bitneedle-format/draft-bitneedle-picture-record-format-03.txt` for the wire format.
