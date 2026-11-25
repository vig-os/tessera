---
type: issue
state: open
created: 2026-11-24T20:42:03Z
updated: 2026-11-25T05:55:23Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/975
comments: 0
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:47.361Z
---

# [Issue 975]: [fix(io): lineages 'a:b' and 'a_b' share one ref file](https://github.com/vig-os/tessera/issues/975)

This bug predates PR #973; its review found it. Lineage names are mapped to ref file names in a way that sends `a:b` and `a_b` to the same file, so two distinct lineages overwrite each other's tip. Since #946 the ref compare-and-set is real, so the second lineage's commits fail with `RefMoved` against a tip that belongs to someone else. Before #946, they silently replaced it.

Expected: lineage-to-file mapping is injective. Either encode reserved characters reversibly (percent-encoding, or a hash plus a stored name), or refuse lineage names outside a documented alphabet when they are created. Write a failing test first: create `a:b` and `a_b` and commit to both.

Refs: #946
