# Getting started

A `.tsra` is a single sealed file — a STORED zip64 holding a JSON manifest plus one or more storage
blocks, all under one blake3 identity. You don't need to unpack it to use it; the `tessera` CLI reads
into the sealed file directly.

The first two things you'll ever do with a product you received are **verify it** and **look at it**:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/getting-started.trycmd}}

`verify` walks the seal *and* re-hashes every block against its recorded digest — an `OK` means the
bytes on disk are exactly what the manifest commits to. `inspect` then shows the identity triple
(`id` / `content_hash` / `manifest_hash`) and the per-block digests that roll up into `content_hash`.

From here: [Reading & navigating](./reading.md) for the full read surface, or
[Anatomy of a `.tsra`](./format-anatomy.md) for what those fields mean.
