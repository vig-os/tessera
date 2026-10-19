---
type: issue
state: open
created: 2026-10-18T12:50:05Z
updated: 2026-10-18T12:50:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/612
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:48.961Z
---

# [Issue 612]: [[ADR-0061 P2a] feat(ingest): de-identification engine skeleton](https://github.com/vig-os/tessera/issues/612)

Phase **P2a** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~250 lines.

- **Scope:** `tessera-ingest::dicom::deid`: `Profile`, `apply(obj) -> (obj', Record, Modified)`, compound-code
  resolution, recursion into sequences, private removal under Basic, R1 groups, R2 unknown public tags, record
  counts. Pure, not wired.
- **Acceptance:** §11.1.6 at object level; a nested private element and a nested group-0004 element are
  removed.
- **Depends on:** P1.

## Dependencies

- P1 → #611

Do not dispatch before its predecessors merge.

