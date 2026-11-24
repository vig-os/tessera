---
type: issue
state: open
created: 2026-11-24T10:39:11Z
updated: 2026-11-24T10:39:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/965
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-24T11:59:38.155Z
---

# [Issue 965]: [Migration record: migrated_from provenance edge with a pixel-equivalence proof, signed by the migrating custodian](https://github.com/vig-os/tessera/issues/965)

## Motivation
When a record is converted (DICOM → .tsra, or .tsra → a denser archival profile #796), Swiss business-record rules (GeBüV Art. 10) require the migration to preserve completeness, correctness and readability, **and to be documented**. Consumers decided that pixel-identical (not byte-identical) re-export is sufficient; that claim has to be provable per product.

## Proposal
- A provenance edge role `migrated_from` pinned to the source's content hash (DICOM: per SOP instance SHA-256; .tsra: `manifest_hash`).
- An **equivalence proof** on the edge: the hash of the canonical decoded pixel stream (and declared header fields) on both sides, plus the decoder identity and version and the transfer syntax of the source.
- The edge is inside the seal; the migration is signed by the custodian performing it (ed25519 now, qualified seal when available).
- A round-trip test suite: DICOM in → .tsra → DICOM out, pixel equality asserted across transfer syntaxes (explicit/implicit VR, JPEG lossless, JPEG-LS, JPEG 2000 lossless, RLE), plus SOP Instance UID handling (keep on pixel-identical re-export; mint new on de-identified export; (0028,2110) unchanged).

## Pitfalls
- Decoder differences can change decoded pixels (photometric interpretation, palette, rescale): canonicalisation must be specified.
- Private tags and non-image objects (#301) need a declared fate per tag (kept, dropped, moved to the identity envelope).

## Acceptance
- [ ] edge role + proof schema; [ ] round-trip corpus passing; [ ] `inspect` shows the migration chain.

Refs #796, #656, #551, #301.
