---
type: issue
state: open
created: 2026-11-27T09:19:37Z
updated: 2026-11-27T09:54:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1029
comments: 1
labels: chore, docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:52.910Z
---

# [Issue 1029]: [docs: remaining raw-id member file names, and wording from the #1026/#1027 reviews](https://github.com/vig-os/tessera/issues/1029)

From the reviews of PR #1026 (#1015) and PR #1027 (#1010):
1. These doc comments and examples still give raw-id member file names that are now wrong:
   - `tessera-io/src/collection.rs` lines 16, 255, 257, 258 and 579 (the `prefix_layout` docs; the code already uses `names::file_name`);
   - `docs/examples/ingest-ge-listmode.toml:8`.

   Update them to the encoded name.
2. Awkward rewrites to smooth out: `tessera-cli/src/collection.rs:670-672` ("…`verify` passes, and member stems use …; `inspect` shows…") and `tessera-ingest/src/engine.rs:1101-1104` (the line break at "Apply the spec's").
3. The CHANGELOG still says the check "refuses missing/empty tokens", and the error message still says "non-empty". Both should say that whitespace-only `fork` tokens count as empty too (#1010).

Refs: #1015
---

# [Comment #1]() by [gerchowl]()

_Posted on November 27, 2026 at 09:54 AM_

One more follow-up, from the review of PR #1025 (#1024). In `tessera-py/src/lib.rs` (~line 311), the `_ => {}` arm required by the `#[non_exhaustive]` `SchemaMismatchKind` sets `column`/`field` to None but leaves `kind`, `expected` and `found` unset. A future kind would then raise `AttributeError`. Set `kind="unknown"` and `expected`/`found` to None in that arm. Also add a Python test of the attributes on the read path (`logical_table`); today they are tested only via `Builder.pack`.

