---
type: issue
state: open
created: 2026-10-18T12:50:17Z
updated: 2026-10-18T12:50:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/619
comments: 0
labels: feature, security, semver:major, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:46.196Z
---

# [Issue 619]: [[ADR-0061 P3c] feat(ingest)!: remove `PHI_TAGS` and catch up the public surface](https://github.com/vig-os/tessera/issues/619)

Phase **P3c** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~250 lines.

- **Scope:** remove `PHI_TAGS` / `phi_present` / `deidentify`; rewrite the CLI help (`--deidentify`,
  `--timestamp`); regenerate the affected trycmd snapshots; rewrite the book and FEATURE-MATRIX line to state
  that the v1 envelope holds slice 1's original header plus the paths only.
- **Acceptance:** trycmd snapshots green; `guardrails-gates` green; no public item named `PHI_TAGS`,
  `phi_present` or `deidentify` remains in `tessera-ingest`.
- **Depends on:** P3b.

## Dependencies

- P3b → #618

Do not dispatch before its predecessors merge.

