# record-cut

Canonical Bitneedle BRS1 record-stream authoring and encoding helpers.

This crate builds valid record streams. The input includes payload descriptors,
payload entries, track mappings, and explicit track-gap ranges. Independent
authoring tools can use its normative write path.

The `descriptor` module contains the BRD1 descriptor-construction helpers. The
`gap` module contains the `GAP1` inter-track silence authoring functions. These
modules implement the wire formats in `record-core` and `record-descriptor`.

## License

This crate is source-available under the Wavey Artist Source Licence.
Individual artists and artist-controlled entities may use it free of charge to
create and sell records containing their own work.
Record labels, platforms, hosted services, technology providers, and other
commercial users require a separate license from Wavey, Inc.
Commercial licensing: license@yl.vin
This crate is not open-source software.
