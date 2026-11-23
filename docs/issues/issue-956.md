---
type: issue
state: open
created: 2026-11-22T20:45:08Z
updated: 2026-11-22T20:45:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/956
comments: 0
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:27.532Z
---

# [Issue 956]: [nits from the #942 round-5 review: pyramid level-0 removal loses the orphan hint](https://github.com/vig-os/tessera/issues/956)

With a primary `v/1.cidx` (or a derived `notes`) that indexes the derived level `v/1`, `commit --remove-block v/0` is still refused. The refusal comes from the seal, though, without the "remove it too" hint that `refuse_orphaned_indexes` gives, because that function checks only blocks named by `--remove-block` and not the blocks the cascade takes. Extend the check to the cascade set and add a test.

Refs: #720
