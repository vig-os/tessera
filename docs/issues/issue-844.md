---
type: issue
state: open
created: 2026-11-13T12:47:43Z
updated: 2026-11-13T12:47:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/844
comments: 0
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-14T08:34:35.088Z
---

# [Issue 844]: [feature(core): check a column contract's own consistency (unique columns, keys/sequence_key/references name declared columns, producer order ⇒ empty sequence_key)](https://github.com/vig-os/tessera/issues/844)

Part of epic #799; follows #809 (ADR-0065 §8).

#809 adds `ProductSchema.tables` / `arrays` and `check_table` / `check_array`, which check a **block** against its role's contract. Nothing checks the **contract** itself, so a malformed one is accepted and then blames valid blocks:
- a contract listing the same column name twice refuses every conforming block as `ColumnOrder`;
- `sequence_key`, `keys` and `references.columns` may name columns the table doesn't declare;
- `order: "producer"` with a non-empty `sequence_key` (ADR-0065 §8 says producer order has an empty key);
- an `ArrayContract` whose `axes` length differs from its `rank`.

## What
A `ProductSchema` self-check (typed error naming the role and the offending name), run where a schema enters: at parse/validate alongside #830's strict rules, and at `tsra_session_open` (ADR-0065 §8 "checks the schema itself").

## Acceptance
- A test per malformed shape above, each failing before.
- The six #801 drafts (`tessera/crates/tessera-core/tests/fixtures/801-consumer-schemas/`) pass the self-check.

Size: ≤ ~150 lines. Depends on: #809.
