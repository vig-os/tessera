---
type: issue
state: open
created: 2026-11-27T13:11:54Z
updated: 2026-11-27T13:11:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1036
comments: 0
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:59.536Z
---

# [Issue 1036]: [nits from the #1033 review (pins), and a question: one bad pin blocks every gc](https://github.com/vig-os/tessera/issues/1036)

From the review of PR #1033 (#1001, ADR-0067 step 6). It found no blockers.

**Design question:** `set_pin` checks that the target is stored and of the right kind, as D4 requires, but it does not check member closure. A pin on a collection whose member is missing is accepted, and every later gc aborts with `GcAborted` until the pin is removed. That matches the ADR, and `set_ref` behaves the same way. Still, one bad pin stops all gc. Consider running the `commit_collection` closure check in `set_pin` (and `set_ref`) for collection targets. That is the proper fix; an ADR-0067 amendment should decide it.

**Nits:**
1. `pins()` hand-writes a percent-decoder (pins.rs:122-136). It is safe because it re-encodes and compares the result, but the decoder belongs next to `tessera_core::names::file_name`, with a round-trip test.
2. If a pin's target was deleted by hand, `remove_pin` fails with a plain `io: No such file or directory`, not `CorruptRef`, and the only way out is deleting the file by hand. Removing a pin should not need its target.
3. `tessera-cli/src/version.rs:640`: the new test was inserted under the #946 doc comment, so `import_into_a_lineage_with_a_version_is_refused` lost its doc and the new test carries the wrong one.
4. `pins_tests.rs` has a 128-character line, and the ADR-0036 amendment has a doubled blank line before its heading.
5. Add a test that a stale `Some(expected)` on `set_pin` is refused. Only one test catches a dropped CAS today.

Refs: #1001
