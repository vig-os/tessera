---
type: issue
state: closed
created: 2026-11-26T20:40:52Z
updated: 2026-11-26T23:03:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/997
comments: 0
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:02.584Z
---

# [Issue 997]: [feat(core): `sources` on Collection with supersedes/forked_from roles (ADR-0067 step 2)](https://github.com/vig-os/tessera/issues/997)

- Add the field to `FIELDS`, with a builder method and the role constants.
- Check the required `fork` token on a forked version.
- Update SPEC §4 and §8, and add seal-case vectors.
- A test proves the corpus does not change.

About 500 lines. Independent of steps 1 and 3.

Implements ADR-0067 (Accepted 2026-11-26, PR #988), implementation plan step 2. Parent: #947.

Refs: #947
