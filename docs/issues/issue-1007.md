---
type: issue
state: open
created: 2026-11-26T22:17:59Z
updated: 2026-11-26T22:17:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1007
comments: 0
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:57.569Z
---

# [Issue 1007]: [fix(io): a crashed repository init cannot be retried, and init writes into a directory it refuses](https://github.com/vig-os/tessera/issues/1007)

Found in the review of PR #1005 (#996, layout 2).

1. `init` creates `refs/lineages`, `refs/pins` and the other directories before it writes the `tessera-repository 2` marker. If it crashes between the two, the next `init` or `open` refuses with "no marker, entries in refs/ (possibly before #975)". That names the wrong cause, and the user has to delete the empty skeleton by hand. Fix: write the marker first (atomically), or treat an unmarked repository that holds only the empty skeleton directories as one `init` may finish.
2. `init` writes a `lock` file into an unmarked directory with entries, and then refuses it. ADR-0067 says nothing is read or written in that case.

Write a failing-first test for each, using the kill-point seam from #946.

Refs: #996
