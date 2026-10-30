---
type: issue
state: open
created: 2026-10-30T01:11:53Z
updated: 2026-10-30T01:23:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/710
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-10-30T08:02:18.140Z
---

# [Issue 710]: [feat(io): resolve numbered-leaf tables in Reader::table_group and LogicalTableView (ADR-0064 PR B)](https://github.com/vig-os/tessera/issues/710)

PR B of ADR-0064's implementation plan (§3, §4, §7).

**Scope (`tessera-io` readers):**
- `Reader::table_group(name) -> Result<TableGroup>` over PR A's resolver; `Reader::logical_table` /
  `LogicalTableView` built on it, with typed `Error::LogicalTable` errors instead of `Error::Container` strings.
- `Reader::block_group` unchanged in behaviour (legacy flat only) and `#[deprecated]`, pointing to `table_group`.
- `select_blocks_overlapping` / `block_stats` iterate the resolved leaves (`pairs/0000.cidx` sidecars).
- `WriteSession::seal`, `unpack` and `pack_dir` (`container.rs:927,1012`, next to their `check_leaf_rule` calls) run
  the shared validation.
- Commit a small legacy fixture written by the **pre-switch** writer, `tessera-io/tests/fixtures/legacy_flat_events.tsra`
  (3 `events_NNNN` blocks via the `with_block_rows` seam, pinned `content_hash`), so the legacy form keeps a real
  product under test after the writers switch (PR D).

**Acceptance (fails before):** `logical_table("pairs")` over a marked `pairs/NNNN` product returns the concatenation
(before: "no blocks for prefix"); `.cidx` pruning skips leaves; the legacy fixture reads identically; an invalid node
returns its typed variant from every reader entry point.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

