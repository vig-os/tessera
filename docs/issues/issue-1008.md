---
type: issue
state: open
created: 2026-11-26T22:18:01Z
updated: 2026-11-26T22:18:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1008
comments: 0
labels: chore, area:testing, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:56.917Z
---

# [Issue 1008]: [nits from the #1005 review (repository layout 2)](https://github.com/vig-os/tessera/issues/1008)

From the fresh review of PR #1005 (#996, merged). It found no blockers.
1. Four of the five baseline tests fail on dev at the `typed_tip` setup read (NotFound), not at their own assertions. Make each one fail on its own assertion.
2. The test names don't match ADR-0067's step-1 test plan: `a_repository_at_another_layout_is_refused_naming_both`, `ref_records_have_one_spelling` and `a_stray_entry_under_refs_aborts_gc`. Rename the tests, or update the ADR.
3. The layout-3 and no-marker refusals are unit-tested on `required` only, not on `found`, and were never shown failing on dev.
4. A CRLF ref record is refused, but no test covers it.
5. `set_ref` records any stored object, blocks included, as `product`. Step 3 (#998) adds the object-kind check; tie the two together.
6. `versioning.trycmd` has no gc, forget or seal step on layout 2.
7. Two lines are over 120 characters: `docs/adr/0036-versioning-audit-object-store.md:229` and `docs/book/src/versioning.md:71` (a URL).
8. The PR body cites a scratch base `5270b355` instead of the real base `4f569cf8`.

Refs: #996
