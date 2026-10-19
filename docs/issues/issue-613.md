---
type: issue
state: open
created: 2026-10-18T12:50:06Z
updated: 2026-10-18T12:50:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/613
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:48.514Z
---

# [Issue 613]: [[ADR-0061 P2a-ii] feat(ingest): UN handling, with no UN ever sealed](https://github.com/vig-os/tessera/issues/613)

Phase **P2a-ii** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~250 lines.

- **Scope:** R8 rules 1-4: re-typing public UN, the Safe Private table lookup, the Implicit VR SQ re-parse
  with the per-item creator lookup, and the no-UN invariant.
- **Acceptance:** §11.1.13(b) at object level.
- **Depends on:** P2a.

## Dependencies

- P2a → #612

Do not dispatch before its predecessors merge.

