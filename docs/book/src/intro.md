# Introduction

**Tessera** is a substrate-agnostic, Rust-native **FAIR data-product format** (`fd5` v2). A Tessera
product is one immutable, content-hashed, self-describing unit: a **manifest spine** plus shape-dispatched
storage **blocks** (dense N-D arrays via Zarr v3 + pcodec; columnar tables via Vortex), sealed into a
single range-readable `.tsra` container.

Identity and integrity are first-class:

- **`id`** — logical identity, `blake3(JCS(id_inputs))`; stable across rename / byte re-encode.
- **`content_hash`** — a recursive **Merkle Mountain Range** root over the ordered block digests
  (per-block and per-chunk inclusion proofs; ADR-0028).
- **`manifest_hash`** — the seal, `blake3(JCS(manifest))`; tampering with any byte is detectable.

This book is the practical how-to. Its command-line examples are **executed by the test suite** (via
`trycmd`) and **included verbatim** here, so what you read is exactly what the tool does — the docs cannot
drift from the behaviour.

Not every capability is a hermetic CLI command, so a chapter's evidence takes one of three forms — and
each chapter says which it is:

- **Executed transcript** — a `{{#include}}` of a `trycmd` walkthrough the CI runs against the real
  binary (most chapters). What you see ran.
- **Executed doctest** — a Rust/Python example run by `cargo test` / the bindings check (the API surface,
  [Bindings](./bindings.md)).
- **Referenced check** — for features that need a live service (an OCI registry, a MinIO bucket, a WASM
  runtime), the example is **shown but marked not-inline-executed**, with a pointer to the Nix flake
  check that *does* exercise it end-to-end. These are called out explicitly so you're never misled that
  a service-dependent snippet ran in-page.
