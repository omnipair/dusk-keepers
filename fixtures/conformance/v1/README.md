# Conformance fixture set v1

`scheduler-cases.json`, `execution-lifecycle-cases.json`, and
`health-provenance.json` are active in both native test suites.
`instruction-envelope-cases.json` adds nine real IDL-derived envelopes and
negative discriminator/account/composition cases. `adapter-codec-cases.json`
adds eleven native Borsh vectors, eight PDA derivations, generated static-account
mappings, and shared invalid-input cases. Snapshot freeze
adds canonical account byte fixtures, fee/math vectors, and instruction-intent
vectors to this versioned directory. Both adapters must consume the same vectors
before either runtime can leave shadow mode.
