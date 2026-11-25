---
type: issue
state: open
created: 2026-11-24T18:54:22Z
updated: 2026-11-24T18:54:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/969
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:49.252Z
---

# [Issue 969]: [spike: leak detection — per-recipient forensic fingerprints in exported data and rendered views, canary records, detection service](https://github.com/vig-os/tessera/issues/969)

## Motivation
Once a consumer decrypts, tessera cannot stop copying. It can make a leak **traceable**, as done for documents, where a per-recipient mark survives into a printout or screenshot. For tessera this means a recipient-specific, keyed fingerprint that survives the common leak paths:
- export → re-share;
- table → CSV;
- image → screenshot or phone photo.

## Layers to spike
1. **Numeric data (tables, arrays):** keyed perturbation below the declared precision of a column (relational watermarking, e.g. Agrawal & Kiernan 2002, to verify), with the column's precision taken from the schema. Exports only: the sealed original stays untouched.
2. **Images and volumes:** robust invisible watermarks (spread-spectrum or transform-domain) embedded in exported or rendered pixels, tested for survival of screenshot, JPEG, resize, crop and phone photo of a screen.
   - **Viewer-level marking** (a per-session mark in the rendered viewport) may be more robust and safer than marking the data.
3. **Canary records:** recipient-unique synthetic rows or instances whose appearance elsewhere identifies the source.
4. **Collusion:** several recipients combining copies to erase marks. Use collusion-resistant codes (e.g. Tardos) and state the limits.
5. **Detection service:** extract a mark from a suspect file or image and look up the recipient in an audited, access-controlled registry.

## Constraints
- **Diagnostic and scientific integrity:** marks must not change clinical or measurement results. Declare a tolerance per data type and test against it. Medical imaging may need reversible watermarking or viewer-only marks.
- **Provenance:** a marked export is a derived product (`derived_from`, role `fingerprinted_for: <recipient>`), never confused with the sealed original. Pixel-identical guarantees apply to unmarked exports only.
- **Privacy:** the recipient registry is itself sensitive and audited.

## Acceptance
- [ ] threat model and the leak paths in scope
- [ ] robustness matrix per layer (survives / degrades / fails) on real images and tables
- [ ] integrity impact measured against the declared tolerances
- [ ] recommendation: which layers ship, in which product profiles

Refs ADR-0055, #968, #966, #583; kit's key-lifecycle spike (time-boxed release). Consumer: nerdmachines/vetgraph.
