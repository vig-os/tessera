---
type: issue
state: open
created: 2026-07-02T12:36:19Z
updated: 2026-09-28T17:59:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/291
comments: 0
labels: dependencies, refactor, priority:medium, effort:medium
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:56.777Z
---

# [Issue 291]: [refactor(io): adopt oci-client + object_store for in-process transport, retire hand-rolled registry.rs](https://github.com/vig-os/tessera/issues/291)

## Context

The `cloud` feature currently ships a **hand-rolled OCI distribution client** (`tessera-io/src/registry.rs`, ~a thin push/pull over `reqwest::blocking` — auth, the `LOCATION`-redirect dance, blob upload). That's Tessera re-implementing a slice of the OCI distribution spec, which is exactly the transport work the format layer should **not** own (it accrues maintenance: chunked uploads, mount APIs, token flows).

This came out of an architecture discussion on where the tsra ↔ storage/distribution boundary should sit (the "thin waist": own the format + proofs + descriptor-builders + prune-brain + verify; delegate byte-movement). Three ways to get OCI push/pull without hand-rolling:

| Option | External binary? | Reimplementation? | Single binary? |
|---|:--:|:--:|:--:|
| shell out to `oras`/`mc` | **yes** | no | no |
| hand-rolled `registry.rs` (today) | no | **yes** | yes |
| **`oci-client` crate** (oras-project) | no | **no** | **yes** |

`oci-client` (formerly `oci-distribution`, maintained by the **oras-project**; used by Fermyon Spin, wasmCloud, krustlet) is "no" on all three liabilities — a maintained, spec-conformant Rust library that runs in-process.

## Decision

- **Adopt `oci-client`** (registry push/pull/index) + keep **`object_store`** (S3 range-read — already in-tree; range-read *can't* be shelled out anyway).
- **Retire the hand-rolled `registry.rs`** — drop bespoke correctness-risk code for the maintained client.
- **Keep shell-out to `oras`/`mc` as an *optional* escape hatch** for minimal-Cargo-footprint builds (loud "install oras" error, never a silent fallback).
- **Own, always:** the OCI manifest/index *shapes* (`oci.rs::artifact_manifest`, `collection.rs::to_oci_index`), the prune-before-fetch brain, and `verify` (the proof). `oci-client` moves bytes; Tessera makes + proves the artifact.

Dependency cost is small: the `cloud` feature already pulls `tokio` + `reqwest` (rustls) + `object_store`; `oci-client` rides the *same* transitive stack, and we **delete** a module in exchange. All stays gated behind `cloud` (default build + wasm core untouched).

## Pre-adoption spike (do first)

- [ ] **Verify `oci-client`'s OCI 1.1 Referrers-API coverage** before leaning on it for the signature/attestation-attach story. push/pull/manifests/auth are known-good; the referrers surface may be partial — confirm or plan a fallback.
- [ ] Confirm the `block_on` wrap keeps the CLI's sync API clean (same pattern already used for `object_store`).
- [ ] `cargo-deny` clears `oci-client` + transitive tree (licenses + advisories).

## Tasks

- [ ] Add `oci-client` behind the `cloud` feature; wrap async in `block_on`.
- [ ] Port `tessera push` / `pull` to `oci-client`; **delete `registry.rs`**.
- [ ] Keep the `oci-roundtrip` flake check green (it already uses a real registry via `oras push` — now the Rust side pushes via `oci-client`, pulls back byte-identical).
- [ ] Wire **collection-as-OCI-index** push/pull (`to_oci_index` already builds the index) with a roundtrip gate.
- [ ] (optional) `--print-cmd` / a `Transport { Oras | Mc }` shell-out escape hatch for minimal-footprint builds.
- [ ] Update the FEATURE-MATRIX OCI row + the book's Distribution chapter (#285) to reflect in-process transport.

## Non-goals

- Implementing/running a registry, object store, WORM enforcement, cache, or DOI — those stay **external substrate** (zot/Harbor, MinIO/S3 Object-Lock, InvenioRDM), forever.
- Growing any bespoke registry-protocol code — if it's not `oci-client` or `object_store`, it doesn't belong here.

## References

- `tessera-io/src/{registry.rs, oci.rs, collection.rs}` · the `oci-roundtrip` / `minio-range-read` flake checks
- ADR-0033 (collections → OCI-index projection) · ADR-0002 (cloud/concurrency) · the pending ownership-boundary / ref-model ADR (thin-waist: own format+proofs, ride OCI+S3)
