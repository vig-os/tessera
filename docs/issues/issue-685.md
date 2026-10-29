---
type: issue
state: closed
created: 2026-10-26T19:45:14Z
updated: 2026-10-29T04:52:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/685
comments: 1
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:04:02.938Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 02:14 AM_

Owner decision (2026-10-29): **block names are hierarchical, and blocks are the leaves of the tree.**

- A name is one or more `/`-separated segments. Each segment matches `[A-Za-z0-9._-]+`, is not `.` or `..`, and the whole name is at most 255 bytes. No leading or trailing `/`, no empty segment.
- **Leaf rule:** within one manifest, no block name may be a parent prefix of another (`volume` and `volume/1` can't both exist), so zip members, exploded directories and S3 prefixes all mirror names one-to-one.
- `tessera pyramid` names level 0 `volume/0` (it was `volume`).
- **Compatibility:** readers accept any name that passes the segment grammar, so older pyramid products still open in-zip. Seal/write enforces the leaf rule. Every filesystem join (`unpack`, `pack_dir`, write-session staging and fragments, `recover`) goes through the validated type, accepts only `Normal` components, and refuses a leaf-rule conflict with a typed error. A name that fails the segment grammar is refused everywhere.
- This defines part of the sealed format, so it gets a short ADR (Accepted, citing this decision), landing in the same PR as the implementation.

Rejected: a single-segment grammar (gives up hierarchy, against ADR-0043, and renames every pyramid level), and a flat on-disk encoding (exploded/S3 layouts would stop mirroring names).

