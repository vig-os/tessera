---
type: issue
state: open
created: 2026-11-03T03:17:34Z
updated: 2026-11-03T03:17:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/744
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:43.087Z
---

# [Issue 744]: [py logical tables: follow-up nits from #740 (C2)](https://github.com/vig-os/tessera/issues/744)

Review nits from PR #740 (ADR-0064 C2, #717; merged db055b9). None blocked the merge.

- [ ] `tessera-py/src/lib.rs` `not_a_block`: it decides "is this a node?" by block-name prefix (`name/`), which is a third copy of the node rule (see #743 item 2), not the core partition-marker rule. A non-partition prefix such as `raw/ct` gets a misleading "a partitioned table reads by its name" hint. Route it through the single core predicate when #743 lands.
- [ ] `read_table`: `data.unwrap_or_default()` returns an empty table with no error if a group ever resolves to zero blocks. Unreachable today, but it is a silent fallback; refuse it with a typed error. It also re-parses specs via `table_spec` after `logical_table` already validated them.
- [ ] `python/tessera/__init__.py` `column(self, block, name)`: the first parameter is still called `block`, but it now takes a logical table name. Rename it (a public API change: CHANGELOG + api_drift).
- [ ] `tests/api_drift.py`: the probe uses a flat group, so `read_block` only covers the "unknown name" path. The node path is covered only in `logical_tables.py`. The "documented" check is a substring match; tighten it.
- [ ] Fixture byte equality is proven on x86 only (the aarch64 leg is nightly). Confirm on the first aarch64 nightly after db055b9 (see #730).

Refs #717, #743

