---
type: issue
state: open
created: 2026-11-24T23:17:03Z
updated: 2026-11-24T23:17:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/978
comments: 0
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:46.065Z
---

# [Issue 978]: [fix(io): a partitioned table whose leaves disagree on nullable seals, then fails on every read](https://github.com/vig-os/tessera/issues/978)

This predates PR #951; its review found it. A partitioned or legacy `events_NNNN` table can be written and sealed with one leaf declaring a column `i4` nullable and another declaring it non-nullable. Every read then fails, in both the Rust reader and Python, with `extend: dtype i4 (non-nullable) != i4 (nullable)`.

Expected: the writer refuses a leaf whose column contract (dtype plus nullability) differs from the table's first leaf, so a product that can never be read is never sealed. Reading stays strict. Start with a failing test that writes the mismatch and expects a refusal at write or seal time.

Refs: #804
