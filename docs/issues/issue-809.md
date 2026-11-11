---
type: issue
state: open
created: 2026-11-11T01:18:41Z
updated: 2026-11-11T04:31:31Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/809
comments: 0
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:37.584Z
---

# [Issue 809]: [feature(core): ProductSchema column contract keyed by role (tables/arrays, optional, sequence_key, keys, references), corpus-neutral (ADR-0065 §8, #801)](https://github.com/vig-os/tessera/issues/809)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Implements the engine half of #801 (gaps G1, G3 and the optional-column flag of docs/spikes/801-consumer-gap-analysis.md).

## What
`ProductSchema` (`tessera-core/src/schema.rs:228`) gains the column contract in the shape of the #801 drafts (`docs/spikes/801-consumer-schemas/*.tables.json`, ADR-0065 §8):
- `tables`: a map **keyed by role**; per role an ordered `columns` list (reusing `Column`: name, dtype, nullable, unit, scale, description) plus `optional: bool` on a column (absent from a product altogether, distinct from per-row NULL), `sequence_key` (#802), `order` (absent = `sequence_key` order; `"producer"` = producer order with an empty key, as the drafts mark single-producer tables), and `keys` (lists of column names) / `references` (#780: carried and sealed; checked only where ADR-0065 §8 says; each entry `columns`, `to { scope, block, columns }`, optional `note`, as in the drafts);
- `arrays`: a map keyed by role (dtype, rank, axes, unit) — ADR-0065's addition; no draft has one;
- both `skip_serializing_if` empty, so every embedded schema sealed so far keeps its bytes; strictness and the `schema_format` bump that stops an older binary from silently dropping them are #830;
- `ProductSchema::check_table(role, &TableSpec)` / `check_array(role, &ArraySpec)` with typed errors naming the role and column.

## Acceptance
- Every builtin schema and the conformance corpus are byte-identical (serialise every builtin, compare with pre-change bytes).
- The #801 drafts parse into the new types: a test merges each of the six `*.schema.json` with its `*.tables.json` `tables` map (five exist; `geddf:pairwise-record` has none; the top-level `product` is checked equal, `status` is draft metadata and dropped) and parses the result under #830's strict rules once #830 lands.
- A test per refusal (unknown role, column order, dtype, nullability, a missing non-optional column, array rank/dtype), each failing before.
- Schema reference docs document the new fields.

Size: ≤ ~400 lines. Depends on: nothing.


