---
type: issue
state: closed
created: 2026-11-26T20:40:50Z
updated: 2026-11-26T22:17:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/996
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:03.368Z
---

# [Issue 996]: [feat(io)!: repository layout 2 with typed, namespaced refs (ADR-0067 step 1)](https://github.com/vig-os/tessera/issues/996)

- Bump the marker to `tessera-repository 2`. Every other layout is refused through #990's `UnsupportedRepositoryLayout`, with `required` = 2 and no migration verb.
- Add `refs/lineages/` and `refs/pins/` with the typed record grammar (product records only at this step), plus `log/lineages/`.
- `InvalidRefName` generalises `InvalidLineage`.
- Remove or retarget #990's `refs/pin:` text and its colon test.
- Update ADR-0036 and the book.

About 900 lines. Independent of steps 2 and 3.

Implements ADR-0067 (Accepted 2026-11-26, PR #988), implementation plan step 1. Parent: #947.

Refs: #947
