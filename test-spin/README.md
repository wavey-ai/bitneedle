# test-spin

Verbose inspection for Bitneedle picture records.

The library renders the decoded BRD1 descriptor, the BRS1 stream and the payload
entries as labeled text for diagnostics. BRD1 and BRS1 stay compact binary wire
formats.

The `record-test` binary runs the same inspection from the command line.

```sh
record-test [--verbose] <record.png> [bundle.json] [manifest]
```

`manifest` is optional. BRD1 embeds a binary `SignedReleaseReference`. Supply
the external manifest to render its human-readable form beside that reference.
