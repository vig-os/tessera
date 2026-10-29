---
type: issue
state: open
created: 2026-10-29T04:52:36Z
updated: 2026-10-29T07:26:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/693
comments: 1
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:04:00.758Z
---

# [Issue 693]: [test(io): backward-compat fixture for a pyramid made by alpha.4 (level 0 as volume/0 rename)](https://github.com/vig-os/tessera/issues/693)

From the PR #691 review: the read-compat test for old pyramid names (`tests/block_names.rs:379`) seals its legacy pyramid by hand. Add a small fixture produced by the released v0.1.0-alpha.4 `tessera pyramid` and assert that the current readers open and verify it.

Refs: #685
---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 07:26 AM_

Also from the PR #699 review (cdea563): list `recover`'s quarantine-before-change test and the seam-guard test as evidence in FEATURE-MATRIX's "Crash recovery" row. Separately: `durable_fragment` checks `blocks.is_dir()` before it creates the directories, so a `blocks/` removed in between makes `create_dir` return a typed `Io` error. That's benign, but say so in the rustdoc.

