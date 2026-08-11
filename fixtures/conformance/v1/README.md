# Conformance fixture set v1

`scheduler-cases.json`, `execution-lifecycle-cases.json`, and
`health-provenance.json` are active in both native test suites. Snapshot freeze
adds canonical account byte fixtures, PDA vectors, fee/math vectors, and
instruction-intent vectors to this versioned directory. Both adapters must consume
the same vectors before either runtime can leave shadow mode.
