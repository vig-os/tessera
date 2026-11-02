---
type: issue
state: open
created: 2026-10-30T01:11:56Z
updated: 2026-11-02T03:45:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/712
comments: 0
labels: feature, area:io, area:ingest
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-11-02T08:51:36.822Z
---

# [Issue 712]: [feat(io)!: multi-block writers emit <table>/NNNN leaves with a partition marker (ADR-0064 PR D)](https://github.com/vig-os/tessera/issues/712)

PR D of ADR-0064's implementation plan (§1, §4, §6). **Breaking change to writer output** (`feat(io)!`).

**Scope:** `table::block_name` names multi-block tables `<table>/{idx:04}` and every leaf's `TableSpec` carries the
partition marker. All multi-block writers go through it and switch together: GE whole-file ingest
(`ge_hdf5.rs:1007`), `TableMultiBlockSink` (`accumulate.rs:575`: GE streaming ingest, `bench --input`) and, if #538
has landed, the generic `ingest table` lanes. Single-block products don't move. Move the tests that assert the old
names (ADR-0064 §6 lists them by path); keep the legacy-reader tests and PR B's legacy fixture.

**Acceptance (fails before):** a multi-block GE ingest and a `TableMultiBlockSink` product have leaves `events/0000…`
with markers and pass the shared validation (before: `events_0000`); streamed and whole-file GE ingest seal equal
block projections (per block, in manifest order: `name`, `kind`, `digest`, `spec.rows`, `spec.row_index`,
`spec.partition`; ADR-0064 §6, since the two paths' description, sources and column annotations legitimately differ)
and the same `content_hash`, and that `content_hash` equals the pre-switch writer's for the same rows (the switch moves
`manifest_hash` only: names and markers aren't in any payload); the comparison is shown failing once against a writer
that misnames a leaf and once against one that drops the marker; the legacy fixture still reads; no conformance-corpus golden moves.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).


