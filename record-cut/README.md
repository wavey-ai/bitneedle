# record-cut

Canonical Bitneedle BRS1 record-stream authoring and encoding helpers.

This crate builds valid record streams from payload descriptors, payload
entries, track mappings, and explicit track-gap ranges. It complements the
public decode and verification crates with the normative write path needed by
independent authoring tools.

It also contains the BRD1 descriptor-construction helpers (`descriptor`
module) and `GAP1` inter-track silence authoring (`gap` module) — the
construction-side counterparts to the wire formats defined in `record-core`
and `record-descriptor`.

## Licence

This crate is source-available under the Wavey Artist Source Licence.
Individual artists and artist-controlled entities may use it free of charge to
create and sell records containing their own work.
Record labels, platforms, hosted services, technology providers, and other
commercial users require a separate licence from Wavey, Inc.
Commercial licensing: licence@yl.vin
This crate is not open-source software.
