---
type: issue
state: open
created: 2026-11-11T04:18:23Z
updated: 2026-11-15T09:23:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/830
comments: 2
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-16T08:59:53.109Z
---

# [Issue 830]: [feature(core): strict ProductSchema (deny_unknown_fields, schema_format) and role-matched block requirements (ADR-0065 §8, G2, G6)](https://github.com/vig-os/tessera/issues/830)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #801 (gaps G2, G6 of docs/spikes/801-consumer-gap-analysis.md).

## What
- **G6:** `#[serde(deny_unknown_fields)]` on `ProductSchema`, `FieldSpec`, `BlockRequirement` and the new table/column contract types (#809), and a `schema_format` integer on `ProductSchema` (absent = 1; the column contract sets 2) that a reader refuses when it is higher than it knows, with a typed error. Today `ProductSchema::from_value` (`tessera-core/src/schema.rs:322`) silently drops unknown keys, so an older binary would validate a v2 schema without its column contract. Binaries already released (≤ alpha.5) cannot be fixed retroactively: document that they validate only `fields`/`blocks` of a v2 schema (the sealed `TableSpec` of every block still carries the columns).
- **G2:** `BlockRequirement` matches **by role**: a block satisfies a requirement only if its role (the table/array role it was written under, recorded with the block) equals the requirement's, not merely its kind (`schema.rs:219-222` today).

## Acceptance
- A v2 schema read by the strict deserialiser round-trips; an unknown key and a too-high `schema_format` are each refused with a typed error (tests failing before the change).
- A product with one `events` table no longer satisfies a required `pethits` (fails before).
- Every builtin schema and the conformance corpus are byte-identical.

Size: ≤ ~300 lines. Depends on: #809.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 15, 2026 at 02:09 AM_

Design note from the #846 review (#809 part A): the column contract reuses `Column`, so a typo inside a contract (e.g. `"optinal": true`) is silently dropped today. Strict parsing here has to make `Column` (and so manifest `TableSpec`s) strict too, or give contracts their own strict column type. Decide which in this issue. Note: #846 uses no `flatten` or `untagged`, so `deny_unknown_fields` remains possible.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 15, 2026 at 09:23 AM_

Fold into this issue (from the #861 review, #809 part B):
- The serialisation byte test misses `ColumnReference.note`. Removing its `skip_serializing_if` passes all tests, because the test only builds a reference that HAS a note. Add a reference without a note to the byte test, before any contract is sealed (#810).
- Naming: `Error::Contract { violation }`. Every sibling (`InvalidBlockName`, `LogicalTable`, `StreamedTable`) calls its sub-error `reason`. Rename it to `reason` while this is still pre-release.

