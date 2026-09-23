# Conformance & the SPEC

A format with one reader is a liability. Tessera's archival promise — *readable decades from now, from
the spec alone* — is backed by a conformance corpus and an independent reader.

- **Conformance corpus** — golden fixtures (`corpus/files/*.tsra`) plus their locked identity hashes
  (`corpus/corpus.json`), checked in CI. A byte drift in any fixture, or a codec/manifest change that
  moves a golden hash, fails the gate — this is the cross-version + cross-arch tripwire.
- **Independent reader** — a second implementation (pure-Python, `corpus/reference_reader/`) written from
  `docs/SPEC.md` alone reproduces all the golden hashes. That's the real test that the SPEC is complete
  and the format isn't secretly defined by the Rust code.
- **Determinism** — writer determinism (same input → same bytes) makes `content_hash` a stable identity,
  and the cross-arch CI matrix (x86_64 + aarch64 Linux) re-derives the goldens on each architecture.

## Release gates

"Shippable" is all four green on the supported matrix (`FEATURE-MATRIX.md` §H): ① conformance corpus ·
② bit-exact roundtrip · ③ perf-SLA floors · ④ writer determinism.

## Maturity — honest status

The format is deliberately **pre-1.0 (v0)**. The correctness and conformance gates are green and a second
reader passes, but load-bearing areas are still maturing (chunked tables, streaming, sub-block Merkle),
so the SPEC is **not yet frozen** — v1.0 means spec-frozen with a 12-month zero-breaking commitment,
which Tessera has not yet made. Use it accordingly, and pin the codec versions (the archival hedge:
vendored readers + spec-pinned pcodec/Vortex).
