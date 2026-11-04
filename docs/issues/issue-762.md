---
type: issue
state: open
created: 2026-11-04T01:25:42Z
updated: 2026-11-04T01:25:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/762
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:50.072Z
---

# [Issue 762]: [feat(io): hdf5-free Rust API for the ADR-0047 identity envelope writer](https://github.com/vig-os/tessera/issues/762)

## Context
MorePET/visionx-rs packs raw PET scanner files into a stage-0 `.tsra` (MorePET/visionx-rs#138, ADR-0001 amendment 2026-11-04, MorePET/visionx-rs#162). The pack must carry an ADR-0047 identity envelope (`aux/identity/`) with the non-allowlisted header fields of every packed file.

## Gap (at `4504efb`)
The ADR-0047 envelope writer `encrypt_identity` lives in `tessera-ingest/src/identity.rs:70`. `tessera-ingest/Cargo.toml:19,22` depends on `hdf5-metno` and its `-sys` crate. A Rust producer that depends only on `tessera-io` and `tessera-core` cannot write an envelope without pulling in libhdf5 and the whole ingest crate.

Relates #566, which covers the Python/CLI `attach-identity` surface. This issue is the Rust library surface.

## Ask
Move the envelope primitives (the identity document type, `encrypt_identity`, and the recipient validation already at `:74`) into an hdf5-free crate, such as `tessera-io` or `tessera-core` or a small `tessera-identity` crate. `tessera-ingest` would then re-export them.

