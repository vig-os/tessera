---
type: issue
state: open
created: 2026-10-25T10:04:07Z
updated: 2026-10-25T10:04:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/663
comments: 0
labels: bug, priority:high, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:23.557Z
---

# [Issue 663]: [fix(io): a panic inside the vortex write task returns Ok with wrong bytes](https://github.com/vig-os/tessera/issues/663)

## Defect

A panic inside vortex's write task does not reach the caller of `table::encode` / `table::encode_streaming`.
The write completes and returns `Ok` with bytes that are silently wrong (truncated or inconsistent with the
input). #658 hit this through a dtype mismatch: an `ArrayStreamAdapter` `debug_assert_eq!` panicked in debug
builds, and a hard `assert_eq!` in vortex-layout's `SequentialStreamAdapter` (`sequence.rs:312`) panicked in
release builds. In both profiles the caller got `Ok`.

PR #662 closes the dtype-mismatch route by validating each row-group in `encode_streaming` before it reaches
vortex. That covers one cause, not the mechanism. Any other assertion or panic inside the vortex write path
still comes back as `Ok(wrong bytes)`, and those bytes are then digested and sealed.

## Ask

Find where the panic is swallowed (the vortex-io current-thread runtime / worker pool that drives the write
task, or the layout writer's spawned futures), and make a panicked write task an `Err` at the tessera boundary.
Possible routes: an upstream fix that propagates `JoinError`-style panics, or a tessera-side guard that detects
an incomplete write.

Test: force a panic inside the write path (e.g. a chunk stream that panics mid-way) and assert that
`encode_streaming` returns `Err`, not `Ok`.

Found in review of #662.

