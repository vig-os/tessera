---
type: issue
state: closed
created: 2026-11-26T20:40:55Z
updated: 2026-11-27T08:51:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/999
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:00.756Z
---

# [Issue 999]: [feat(io): collection objects in the repository and gc through members (ADR-0067 step 4)](https://github.com/vig-os/tessera/issues/999)

- `put_collection` / `get_collection`, and collection records.
- The record/object kind check, and `walk_read` dispatching by kind.
- Members are closed within the repository, with member-path errors.

About 1000 lines. Needs steps 1–3 (#996 #997 #998). #994 should land first.

Implements ADR-0067 (Accepted 2026-11-26, PR #988), implementation plan step 4. Parent: #947.

Refs: #947
