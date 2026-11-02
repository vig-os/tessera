---
type: issue
state: open
created: 2026-10-30T01:11:52Z
updated: 2026-11-02T03:45:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/709
comments: 0
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-11-02T08:51:37.779Z
---

# [Issue 709]: [feat(core): partition marker and shared logical-table resolution (ADR-0064 PR A)](https://github.com/vig-os/tessera/issues/709)

PR A of ADR-0064's implementation plan (§2, §3).

**Scope (`tessera-core` only):**
- `TableSpec.partition: Option<Partition { index: u64, last: bool }>`, skipped when `None` (single-block specs
  serialize byte-identically; corpus untouched).
- `LogicalTableError` (`#[non_exhaustive]`: `NotFound`, `NotATable`, `Ambiguous`, `ForeignChild`, `NonCanonicalLeaf`,
  `StrayMarker`, `SingleLeaf`, `DuplicateIndex`, `Gap`, `NoTerminal`, `AfterTerminal`, `OutOfOrder`, `SchemaMismatch`) and `Error::LogicalTable { name, reason }`.
- One shared, manifest-only resolver/validator for a table name: partitioned node (§2.1), legacy flat `_NNNN` rule
  moved verbatim from `Reader::block_group`, the ambiguity rule (§3, §4), and `leaf_name(prefix, index)`
  (`format!("{index:04}")`).
- `PARTITION_CHILD_CLASSES`: the closed list of child classes allowed under a partitioned node (ADR-0064 §2.1), today
  only the `<table>/<leaf>.cidx` sidecar of an existing leaf.
- Manifest order checked on read as well as at seal: leaves in strictly increasing index order, else `OutOfOrder` (§2.5).
- `ProductBuilder::seal` validates every node holding a marked leaf (one segment below the
  node), stray markers, `k ≥ 1`, the mixed-form rule and index-ordered leaves (ADR-0064 §2.4, §2.5; the `tessera-io` call sites are PR B).

**Acceptance (each test fails before the PR):** a sealed `pairs/0000..0002` (marked) plus an unmarked `pairs/0003`
is refused (`ForeignChild`); gap, duplicate, missing terminal, after-terminal, non-canonical `pairs/00003`, and mixed
`pairs_0000` + `pairs/0000` (`Ambiguous`) each yield their variant, as do leaves listed `pairs/0001` before `pairs/0000` (`OutOfOrder`), a marker on a top-level or legacy flat block such as `pairs_0001` (`StrayMarker`) and a one-leaf partition
(`SingleLeaf`); a `pairs/0000.cidx` sidecar is accepted and a `pairs/view` array is `ForeignChild`; a marked leaf named in full (`pairs/0003`) resolves `Single`; a legacy `events_0000..0002` product with an unrelated unmarked `events/notes` blob still resolves
`events` as legacy flat; index 10000 is named `10000` and orders after
9999; the legacy rule matches exactly what `block_group` matched (the cases of `container.rs:1877-1916`).

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).


