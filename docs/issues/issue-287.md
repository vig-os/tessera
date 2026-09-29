---
type: issue
state: open
created: 2026-07-02T12:13:51Z
updated: 2026-09-28T18:00:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/287
comments: 1
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:58.180Z
---

# [Issue 287]: [ADR: storage-format posture — sealed product container + OME-NGFF at the array layer (not Icechunk/OME-Zarr as the format)](https://github.com/vig-os/tessera/issues/287)

Capture the load-bearing storage decision from the explorer design spike (docs/spikes/tsra-explorer.md (Prior art & reuse), #286) as an ADR.

**Decision:** OME-Zarr is a *store* (keyed bag of chunk-objects); `.tsra` is a *product* (one sealed, content-addressed, signed, versioned file of heterogeneous blocks). The differences are **essential at the container level** (single sealed file, content-addressing, Merkle seal, ed25519, Vortex tables, provenance, WORM/PHI) and **incidental at the array-chunk level** (tsra arrays are already zarr v3).

- **Adopt** OME-NGFF **at the Array-block layer** + expose a `tsra serve` NGFF store-facade (chunk-key → in-container range read; chunk index = offset map) so viv/neuroglancer/vtk.js/napari read it unmodified.
- **Don't adopt** Icechunk or OME-Zarr **as the container format** — would dissolve the differentiators.
- Borrow / bridge Icechunk separately (see linked issues).

Refs: #286
---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 02:33 PM_

Recorded as **ADR-0050** (`docs/adr/0050-storage-format-interop-posture.md`, Status: Proposed) on `feature/286-tsra-explorer` — commit f652f19. (Numbered 0050 because 0047–0049 are taken on parallel branches: crypto-shred, provenance-edge, recursive-collections.) Decision: keep the sealed-product container; adopt OME-NGFF at the Array-block layer via a read-only `tsra serve` store facade; don't adopt Icechunk/OME-Zarr as the format; borrow Icechunk txn (#288) + virtual-chunk (#289); optionally bridge (#290).

