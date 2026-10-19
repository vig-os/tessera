---
type: issue
state: open
created: 2026-10-18T12:50:10Z
updated: 2026-10-18T12:50:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/615
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:47.739Z
---

# [Issue 615]: [[ADR-0061 P2b-ii] feat(ingest): attestation attributes and the decoding oracle](https://github.com/vig-os/tessera/issues/615)

Phase **P2b-ii** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~250 lines.

- **Scope:** the attestation attributes (multi-valued (0012,0063) including the §3.0 extra value, (0012,0064),
  (0028,0303)); the `value_to_json` inverse; Oracle A with check (d).
- **Acceptance:** the oracle round-trip test for every VR; §11.1.8 and §11.1.12(c) at object level.
- **Depends on:** P2b.

## Dependencies

- P2b → #614

Do not dispatch before its predecessors merge.

