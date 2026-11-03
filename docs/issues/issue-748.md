---
type: issue
state: open
created: 2026-11-03T08:18:30Z
updated: 2026-11-03T08:18:30Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/748
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:42.172Z
---

# [Issue 748]: [push_table: follow-up nits from #747 (E1), incl. EIO reported as truncated](https://github.com/vig-os/tessera/issues/748)

Review nits from PR #747 (ADR-0064 E1, push_table; merged fd9b346). None blocked the merge. E2's barrier test pins the exact counter bound and the free-worker gate; the items below are what remains outside it.

- [ ] **Bug-class:** `tessera-io/src/accumulate.rs` (~75-77, ~91-93, ~111-113) maps every `read_exact` error to "fragment: truncated …", including real I/O errors such as EIO. Map only `UnexpectedEof` to truncated, and propagate the others as I/O errors. Add a test with a reader that returns a non-EOF error.
- [ ] **Cleanup tests:** add a worker-error test (a corrupt or unreadable staged input) and a dropped-mid-stream test, each asserting that `staging/` is empty. In the TooFewRows and OversizedGroup cases no staged input file exists when the error fires, so their "staging empty" checks are weaker than the test names imply. Rename them or strengthen them.
- [ ] **Empty table:** `spec.rows == 0` goes through `dispatch`'s empty-table branch. Pin it byte-identical to `table_job_streaming`.
- [ ] **Casts:** replace `expect("still being filled")` and the `as i64` / `as u64` casts in `StagedInput::{append,into_job,Drop}` with `try_from` and typed errors.
- [ ] **Rustdoc:** state plainly that a failed multi-block `push_table` leaves the whole session unsealable (leaves already handed over stay committed, per ADR-0064 §5.3), not only that table.

Refs #713

