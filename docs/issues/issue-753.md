---
type: issue
state: open
created: 2026-11-03T13:13:25Z
updated: 2026-11-03T13:13:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/753
comments: 0
labels: bug, priority:medium, area:ingest, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:55.109Z
---

# [Issue 753]: [bug(ingest): NIfTI uint8 seals as uint16 while .npy keeps uint8 — same data, different identity](https://github.com/vig-os/tessera/issues/753)

Found by an independent SOLID/DRY review of dev 74fc409 (astra + Fable, cross-checked by a third verifier). Confirmed by the verifier.

## Defect: the same uint8 data seals with a different dtype and hash, depending on the ingest lane
- `tessera-ingest/src/nifti.rs` ~745-747 reads NIfTI uint8 as `U16`. Its comment, "Tessera's dtype floor is 16-bit (ADR — 8-bit out of scope)", is stale.
- `npy.rs` ~508 keeps `('u', 1) => "u1"`, which becomes `"uint8"` (`io/array.rs` ~143), and `core/dtype.rs` ~16 allows `UInt8`.
- So the same uint8 mask seals as `"uint16"` via `.nii` and as `"uint8"` via `.npy`: a different manifest spec and different hashes.
- The test at `nifti.rs` ~1688 locks in the widening.

## Fix path
Decide the rule once: uint8 is a supported dtype (#418 closed the envelope), so NIfTI should keep `uint8`. This moves the `content_hash`/`manifest_hash` of existing NIfTI uint8 products, so it needs an ADR note and a declared corpus event (ADR-0052). Consider bundling it with the ADR-0059 format event (#720).

Add a test that fails today: the same uint8 array ingested via `.npy` and `.nii` seals an equal array dtype and data digest.

