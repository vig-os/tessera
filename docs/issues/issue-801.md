---
type: issue
state: open
created: 2026-11-11T01:02:42Z
updated: 2026-11-11T01:26:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/801
comments: 1
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:41.938Z
---

# [Issue 801]: [feature(schema): a registered product schema per C++-written product type, validated at the bridge](https://github.com/vig-os/tessera/issues/801)

Sub-issue 2 of #799. Schemas are data in the product-schema registry, not engine code; the bridge validates every batch against the declared schema.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 01:26 AM_

Gap analysis: [801-consumer-gap-analysis.md](https://github.com/vig-os/tessera/blob/docs/801-consumer-gap-analysis/docs/spikes/801-consumer-gap-analysis.md) (PR #820). The draft schemas are in `docs/spikes/801-consumer-schemas/`.
1. Every consumer product is flat and fixed-width. Each maps onto today's `ColumnData` once tuples are expanded to scalar columns (#768) and NaN or 0 sentinels become NULLs.
2. A `ProductSchema` has no column contract (`schema.rs:225-243`), and `BlockRequirement::matches` ignores the role (`:220-222`).
3. `WriteSession` can't embed a custom schema, seal never validates one, and unknown schema keys are silently dropped (G4-G6). Enum columns have no code table (G13).
4. Drafts for six products: a `ProductSchema` as serde writes it, plus a proposed per-role `Column` contract; `check.py` checks them.


