---
type: issue
state: open
created: 2026-10-18T12:50:19Z
updated: 2026-10-18T12:50:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/621
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:45.459Z
---

# [Issue 621]: [[ADR-0061 P4] feat(ingest): identity envelope v2](https://github.com/vig-os/tessera/issues/621)

Phase **P4** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** `IdentityDocument` v2 (per-slice modified attributes, the UID map, Δ when present, source and spec
  paths) and `reidentify` reading v1 and v2; the slice-1 doc line becomes the full claim.
- **Acceptance:** §11.1.9(a) and (c) under Basic.
- **Depends on:** P3b.

## Dependencies

- P3b → #618

Do not dispatch before its predecessors merge.

