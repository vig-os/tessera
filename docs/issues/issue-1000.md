---
type: issue
state: open
created: 2026-11-26T20:40:56Z
updated: 2026-11-27T09:09:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1000
comments: 1
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:00.119Z
---

# [Issue 1000]: [feat(io): commit_collection, log and history (ADR-0067 step 5)](https://github.com/vig-os/tessera/issues/1000)

- CAS on the ref.
- Lineage kind checks in both directions, and the lineage id check.
- Member closure, including new members added in the same call.
- The history walk over `supersedes` and `forked_from`.

About 1100 lines. Needs #999.

Implements ADR-0067 (Accepted 2026-11-26, PR #988), implementation plan step 5. Parent: #947.

Refs: #947
---

# [Comment #1]() by [gerchowl]()

_Posted on November 27, 2026 at 08:57 AM_

**Architect ruling, 2026-11-27, on the open T5 contract:**
- `Repository::history(address)` returns the distinct reachable version records. Each record has the address, the object kind, the lineage id and labelled predecessor edges.
- The walk is depth-first preorder from the requested version, following `supersedes` before `forked_from`, and visits each object once.
- A missing or unreadable predecessor is a **typed error** that names the predecessor and the edge leading to it. History is never silently truncated. This mirrors gc's abort-on-unreadable rule (#946, #1022).
- Rules that already exist on dev (the fork token, #1004; lineage kinds, #1022) are covered by their existing tests, cited by name in the PR body.

The PR records this as a dated clarification in ADR-0067 under T5.

