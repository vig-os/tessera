---
type: issue
state: closed
created: 2026-06-26T13:45:09Z
updated: 2026-09-28T17:54:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/215
comments: 2
labels: area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:03.985Z
---

# [Issue 215]: [The unified hierarchy: recursive MMR Merkle + multiscale {hash,stats} pyramid + derived sidecars + fused streaming](https://github.com/vig-os/tessera/issues/215)

## The keystone
Several threads converge into one design — a **uniform, content-addressed, multiscale hierarchy built in a single streaming pass and verifiable at every level**. Captured in ADR-0028 (supersedes the flat-list root of ADR-0020, absorbs ADR-0027).

## Decisions
1. **Recursive Merkle tree, not a flat list.** Today `content_hash = blake3(concat(block_digests))` — one level. Replace with ONE recursive node-hash construction `product → blocks → chunks → sub-chunks`, so inclusion proofs work at every level with one verifier. (A deliberate pre-1.0 / v0.2 `content_hash` identity change → golden regen; reader is digest/Merkle-based so it adapts.)
2. **Append-friendly at the streaming level (MMR / CT history tree).** Gives **consistency proofs**: the sealed root is provably an append-only extension of the live root verified at watermark T — the DAQ guarantee ('the archive is the capture I watched'). Balanced Merkle is fine for static levels; one MMR-shaped construction throughout for uniformity.
3. **Each node = `{ hash, rolled-up monoid stats }` = the multiscale pyramid.** Integrity + pruning + overview share the tree. Arrays = OME-Zarr spatial downsample pyramid (interop); tables = aggregate pyramid (the ADR-0027 monoid tree). One 'give me level L' across substrates.
4. **Derived-sidecar block class.** Pyramids / projections (MIP/MPR, group-bys, sort-indexes) / indexes / thumbnails / format-views: pure function of canonical data, regenerable, content-addressed, **recipe-stamped** (`from = <digest>`, `recipe = <how>`), **NOT in the canonical identity** (own digest, separate tier), optional/detachable, stored in the same two substrates. Manifest **tags each block canonical|derived** so identity stays clean.
5. **Fused streaming pass.** ENCODE POOL (parallel, per-chunk): pcodec/zstd|Vortex encode + blake3(encoded bytes, hot in cache) + min/max/count/sum over raw — one touch while resident in the ring. ORDERED COMMITTER (serial): durable fragment + MMR-append leaf + fold stats up the tree + advance the live root. Push-ordered → deterministic == batch; float sum needs canonical reduction.

## Done-when
- ADR-0028 accepted; SPEC: recursive node-hash + MMR rules + sidecar class + recipe schema (+ the ADR-0027 stat registry).
- tessera-core: recursive/MMR Merkle + inclusion + consistency proofs; canonical/derived block tagging.
- tessera-io: the fused encode+hash+tree+stats pipeline (on the ADR-0026 accumulator); derived-sidecar emit/verify.
- Conformance: multi-level proof fixtures + a derived-sidecar fixture; golden regen for the new root.
- Relates: supersedes ADR-0020 flat root, absorbs ADR-0027, rides ADR-0026; gated on #198 determinism.
---

# [Comment #1]() by [gerchowl]()

_Posted on June 26, 2026 at 04:48 PM_

**ADR-0028 §1–2 (recursive MMR `content_hash`) IMPLEMENTED** (`0847c29`).

`tessera_core::hash`: domain-separated leaf(`0x00`)/node(`0x01`), peaks binary-carry, bag-the-peaks right-to-left. Replaces the flat `blake3(concat(digests))` (ADR-0020) → recursive MMR root; **v0.2 identity revision** (content_hash + manifest_hash shift; id unchanged). MMR stays inherently incremental → `MerkleAccumulator` streaming root == batch root at every watermark.

**Triple cross-validated:** 8 Rust unit tests (incl. non-tautological recursive-tree structural proof); conformance 3/3 (corpus.json + corpus/files/*.tsra regenerated); **independent pure-Python reference reader 6/6** (SPEC-only — proves reproducibility from spec, not just code). SPEC §3 + reference_reader ported to MMR. Fresh-context reviewer APPROVED.

**Still pending for ADR-0028 (stays Proposed):** `{hash,stats}` chunk-index block (§3, was #214/ADR-0027) ← next, where #221-B leaf-granularity gets measured; inclusion/consistency proofs; multiscale pyramid; derived sidecars; fused streaming pass.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0028 accepted; ADR-0043 (unified recursive hierarchy) landed 6acfa52 tracking #215; pyramid landed 501230c/c5ad92b (#277). Multiscale + derived-sidecar architecture is now the trunk model.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

