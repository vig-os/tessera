---
type: issue
state: open
created: 2026-11-03T08:18:30Z
updated: 2026-11-03T12:40:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/748
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:56.120Z
---

# [Issue 748]: [push_table: follow-up nits from #747 (E1), incl. EIO reported as truncated](https://github.com/vig-os/tessera/issues/748)

Review nits from PR #747 (ADR-0064 E1, push_table; merged fd9b346). None blocked the merge. E2's barrier test pins the exact counter bound and the free-worker gate; the items below are what remains outside it.

- [ ] **Bug-class:** `tessera-io/src/accumulate.rs` (~75-77, ~91-93, ~111-113) maps every `read_exact` error to "fragment: truncated …", including real I/O errors such as EIO. Map only `UnexpectedEof` to truncated, and propagate the others as I/O errors. Add a test with a reader that returns a non-EOF error.
- [ ] **Cleanup tests:** add a worker-error test (a corrupt or unreadable staged input) and a dropped-mid-stream test, each asserting that `staging/` is empty. In the TooFewRows and OversizedGroup cases no staged input file exists when the error fires, so their "staging empty" checks are weaker than the test names imply. Rename them or strengthen them.
- [ ] **Empty table:** `spec.rows == 0` goes through `dispatch`'s empty-table branch. Pin it byte-identical to `table_job_streaming`.
- [ ] **Casts:** replace `expect("still being filled")` and the `as i64` / `as u64` casts in `StagedInput::{append,into_job,Drop}` with `try_from` and typed errors.
- [ ] **Rustdoc:** state plainly that a failed multi-block `push_table` leaves the whole session unsealable (leaves already handed over stay committed, per ADR-0064 §5.3), not only that table.

Refs #713

---

# [Comment #1]() by [gerchowl]()

_Posted on November 3, 2026 at 10:00 AM_

More nits from the review of PR #749 (E2, ReadHold seam; merged). None blocked the merge.
- [ ] `stream.rs` ~1033-1039 / ~701: `finish` hangs forever if a `ReadHold` is still alive, and so does `push_table` on the holding thread when it needs a free worker, because held jobs wait in `wait_read_turn` and the committer never exits. Either document it ("drop the hold before `finish`; `push_table` must run on another thread") or have `finish` clear the hold. Preferred: `finish` refuses with a typed error while a hold is alive, instead of hanging.
- [ ] `stream.rs` ~520-524: the hold is one `bool`, so dropping one of two `ReadHold`s releases both. Count the holds, or make a second hold a typed error.
- [ ] The barrier test monitor (~2155-2163) spins forever, instead of failing, if a regression blocks the reader somewhere that neither raises `producer_stalls` nor pulls past the limit. Consider a bounded spin that fails with a diagnostic (a bound on iterations of a deterministic condition, not a wall-clock sleep).

---

# [Comment #2]() by [gerchowl]()

_Posted on November 3, 2026 at 12:40 PM_

From the review of PR #750 (E3): the panicking-sibling test (`stream.rs` ~1916, the #680 property) now goes through `push_table`, which stages each block on the calling thread before handing it to a worker. That makes it likelier the panic closes the window before any sibling is encoding, so the "a sibling's that was still encoding" case may never be exercised, and `let _ =` hides that. The race already existed with `push_staged`. Add an assertion that at least one sibling was accepted or encoding when the panic fired, by a deterministic signal, not timing.

