---
type: issue
state: open
created: 2026-07-02T14:14:21Z
updated: 2026-09-28T17:59:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/297
comments: 1
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:54.204Z
---

# [Issue 297]: [T4: cross-object aggregated analysis + query-engine ladder (object-table levels)](https://github.com/vig-os/tessera/issues/297)

**Tier 4** of the compute topology (EPIC #295). Population/cohort analytics over the **derived/aggregate** table — small PHI-safe per-object summary rows unioned across many products (the fan-in from T3).

**Object-table levels (the principle):** intra-object raw tables (listmode) are huge but object-local → stay **inside the sealed product** (Vortex), queried by **DataFusion embedded**. Inter-object derived tables are the analytics level here.

**Engine ladder (Arrow is the waist — no lock-in):**
- **DataFusion embedded** over Vortex — default, single binary, intra-object + query-a-product/small-cohort
- **DuckDB** — external consumer over the Arrow/Parquet projection, interactive analyst/notebook (NOT a second embedded engine — DRY guardrail)
- **lakehouse (Databricks/Snowflake)** — only at genuine population/multi-node/governed scale
- **Ballista** — distributed shuffle (see #, its own spike)

Includes the projection ELT (per product → PHI-safe summary row → union) and the catalog/index integration (see T5 #). Refs: #286
---

# [Comment #1]() by [gerchowl]()

_Posted on July 3, 2026 at 10:44 AM_

View-model aggregation primitives (this belongs with T4): histogram landed in tessera-explore (commit 9b81197, dense rescale-aware two-pass → Arrow RecordBatch). Still deferred from the spike-doc's set: density2d (same kernel over two variables) and downsample (already exists as tessera_io::array::downsample_* — just needs a RecordBatch-returning wrapper). Small follow-ons.

