---
type: issue
state: open
created: 2026-10-18T12:50:13Z
updated: 2026-10-18T12:50:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/617
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:47.085Z
---

# [Issue 617]: [[ADR-0061 P3a] fix(ingest): opaque provenance edges and pixel gates on today's path](https://github.com/vig-os/tessera/issues/617)

Phase **P3a** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~350 lines.

- **Scope:** the §9.5 redacted spec edge with `--print-spec-hash`; the §9.2 opaque source reference for every
  member of a de-identifying run; the two additive v1 envelope fields, with `reidentify` returning them; the §8
  refusals with their surface (the `pixel_review` product key and `--deid-pixel-review`). All of it applies to
  the existing `--deidentify` / `--recipient` paths, adds no attestation and only tightens.
- **Acceptance:** §11.1.5(a) and (b), and §11.1.7, end to end; `reidentify` returns the paths from a v1
  envelope; a mixed DICOM+NIfTI de-identifying run seals no path.
- **Depends on:** none (lands before P3b; can run in parallel with P1-P2c).
- **Split if needed:** the §8 pixel gates as a second PR after the provenance edges.

## Dependencies

None.

Do not dispatch before its predecessors merge.

