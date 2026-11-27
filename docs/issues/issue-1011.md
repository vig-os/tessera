---
type: issue
state: closed
created: 2026-11-26T23:06:18Z
updated: 2026-11-27T06:24:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1011
comments: 0
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:55.642Z
---

# [Issue 1011]: [fix(io): table leaves that differ in unit, scale or referencing concatenate silently](https://github.com/vig-os/tessera/issues/1011)

Found in the review of PR #1009 (#978). The leaf-contract check compares column name, order, dtype and nullability. Column metadata such as `scale`, `unit` and `referencing` is ignored on both the seal side and the read side. Those fields change what a value means, so leaves recorded in ns and in ms concatenate into one column without any error.

Decide which metadata belongs to the value contract: anything that changes interpretation, as opposed to purely descriptive text. Then have both the seal check and `extend` refuse a mismatch in it, through the one shared comparison from #978. Write a failing test first.

Refs: #978
