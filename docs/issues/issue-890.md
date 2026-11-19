---
type: issue
state: open
created: 2026-11-16T10:56:36Z
updated: 2026-11-18T14:51:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/890
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:12.091Z
---

# [Issue 890]: [test(io): every_advance_of_next_restarts_the_stall_deadline fails under a loaded host](https://github.com/vig-os/tessera/issues/890)

`tessera-io` `stream::sequenced::tests::every_advance_of_next_restarts_the_stall_deadline` failed once (6.13 s) in a `cargo nextest run -p tessera-core -p tessera-io` on the shared [batch-host] seat, with a parallel cargo build running, on branch `fix/873-collection-schema-forward-compat` (no change to `tessera-io`). The rerun passed 511/511, and the test alone passed 3/3 (2.75 s each).

The test sleeps `STALL * 9 / 20` (0.9 s) between advances against a 2 s stall deadline, so a scheduling delay of about 1.1 s on one sleep fires the deadline. A loaded host can cause that. A wall-clock test can't separate a code bug from host load. The fix is to make the test independent of host load (an injectable clock), not to widen the timing, so it gives the same answer on CI and on a busy seat.

Observed: 2026-11-16, nextest output `FAIL [ 6.130s] (339/511) tessera-io stream::sequenced::tests::every_advance_of_next_restarts_the_stall_deadline`.
