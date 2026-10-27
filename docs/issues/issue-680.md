---
type: issue
state: open
created: 2026-10-26T11:41:37Z
updated: 2026-10-26T19:47:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/680
comments: 1
labels: chore, priority:low, effort:small, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-27T07:16:20.866Z
---

# [Issue 680]: [test(io): StreamWriter credit-window follow-ups from the #679 review](https://github.com/vig-os/tessera/issues/680)

Follow-ups from the review of #679 (credit window, part 1 of #677), merged at `d3bf9f9`. None of these is a
correctness defect; the deadlock, error-path and byte-bound probes all held.

1. **Test (b) can't show its own pre-fix failure** (`stream.rs` ~793). It reads private internals (`sw.window`,
   `WindowState`), so it doesn't compile against the old code, and the "fails before the fix" evidence came from a
   separate public-API run that isn't in the repo. Add a public-API variant, using `StreamStats::producer_stalls` /
   `peak_in_flight_jobs`, that would also compile and fail on the pre-fix pipeline.
2. **`spin_until` has no upper bound** (~773). A regression hangs CI until the job timeout instead of failing. Add a
   generous watchdog, such as a deadline that only turns a hang into a failure and makes no timing assertion.
3. **`finish_with_stats` returns at the first worker panic without joining the other threads** (~557). This predates
   #679; the threads exit because the window closes. Join every thread, then report the first error.
4. **`StreamStats::producer_stalls` is `u64`** while the other counters are `usize`. Pick one type before the struct is
   relied on (it is `#[non_exhaustive]`, but changing a field type is still a break).

Do it with part 2 of #677 if that PR touches these lines, otherwise as its own small PR.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 26, 2026 at 07:47 PM_

Two more #677 follow-up nits from the #682 round-2 review: (1) the poisoning test injects only an fsync failure; add a seam for a failed or short journal `writeln!` so the other half of the claim is pinned too. (2) The module doc in `write.rs:19-20` has lines well over 100 columns.

