---
type: issue
state: open
created: 2026-10-18T12:50:18Z
updated: 2026-10-18T12:50:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/620
comments: 0
labels: feature, security, semver:major, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:45.836Z
---

# [Issue 620]: [[ADR-0061 P3d] feat(ingest)!: de-identify DICOM by default (decision D1)](https://github.com/vig-os/tessera/issues/620)

Phase **P3d** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~200 lines.

- **Scope:** no flag means Basic; `--keep-identity` / `keep_identity = true` (exclusive with every
  de-identification key) keeps today's keep-PHI path and its warning; the docs follow.
- **Acceptance:** a flagless ingest is de-identified (Oracle A); `--keep-identity` reproduces the pre-flip
  bytes; combining it with a de-identification key is a hard error.
- **Depends on:** P3c.

## Dependencies

- P3c → #619

Do not dispatch before its predecessors merge.

