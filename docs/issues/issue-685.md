---
type: issue
state: open
created: 2026-10-26T19:45:14Z
updated: 2026-10-26T19:45:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/685
comments: 0
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-27T07:16:18.635Z
---

# [Issue 685]: [fix(io): WriteSession joins unvalidated block names into the session path (path traversal)](https://github.com/vig-os/tessera/issues/685)

Found during the review of #682 (#677 PR 2); it was already on dev at 41de440.

`WriteSession::append_block` creates the fragment at `self.dir.join(BLOCKS).join(&block_ref.name)`
(`tessera/crates/tessera-io/src/write.rs:215`). `recover` (`:280`) and the fragment iterator (`:359`) do the same.
`block_ref.name` is never validated. With `Path::join`, a name containing `../` escapes `blocks/`, and an **absolute**
name replaces the base entirely. So a block name taken from caller input (an ingest spec, `commit --add-block`, the
Python/stream APIs) can create or overwrite a file anywhere the process can write, and `recover` will read it back.

## Fix (root cause, not per call site)
- Give block names one validated type, or one constructor, at the boundary: the same grammar the manifest accepts for a
  block name (check `tessera-core`'s rules). Refuse an empty name, `.`, `..`, a path separator, an absolute path, a NUL
  byte and a Windows drive or `\`. Every `join` of a block name goes through it.
- `recover` must refuse a journal line whose name fails validation; it must not follow it.
- Test: each bad shape is refused at append and at recover, and no file is created outside the session dir.
