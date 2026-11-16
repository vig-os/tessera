---
type: issue
state: open
created: 2026-11-15T20:36:45Z
updated: 2026-11-15T20:36:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/877
comments: 0
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:36.380Z
---

# [Issue 877]: [test(io): #472 guard misses a per-array float sum stored in a non-raw encoding](https://github.com/vig-os/tessera/issues/877)

From the PR #788 re-review (head 3b1337f). The #472 guard in `table.rs` (~:1993–2024) scans for the two default NaN bit patterns in raw stat bytes. Its doc admits one gap: a per-array sum stored in a non-raw encoding would not be caught.

What limits the risk today:
- vortex#10122 canonicalises a NaN sum when stats are serialised.
- Zone maps are asserted to carry no sum.
- The cross-architecture corpus comparison is the end-to-end check.

Close the gap, either by decoding the stats before checking or by asserting that no float sum stat exists at all, or record why it can't arise.

Refs: #635
