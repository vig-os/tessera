---
type: issue
state: open
created: 2026-09-29T06:57:36Z
updated: 2026-09-29T06:57:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/502
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:22.569Z
---

# [Issue 502]: [ingest: Parquet footer null_count fast path for streaming pass 1, gated on proof-of-agreement](https://github.com/vig-os/tessera/issues/502)

## What

#458's streaming table ingest uses a **two-pass** design: pass 1 records, per leaf, whether any row is
null; pass 2 encodes with nullability fixed. Pass 1 is needed because nullable-by-presence (ADR-0029) makes
a column's sealed representation a function of the whole file — `ColumnData::Nullable` is a representation,
not a flag, and the multi-block sink requires one schema across blocks, so the first block cannot be
encoded until the last row has been read.

For **Parquet specifically**, pass 1 could in principle be nearly free: each column chunk's
`null_count` lives in the footer statistics, so the whole-file null observation is metadata-only — no page
decode at all.

## Why it was NOT done in #458

Parquet statistics are **optional**, and they are the **producer's claim**, not something we verify. If a
writer omits them, or emits a wrong `null_count`, the seal's nullability — and therefore `content_hash` —
changes silently, and the resulting product disagrees with what a batch ingest of the same bytes would
produce.

That is the wrong direction for this format. The decoder-digest work (#477 / ADR-0056 §6a) turned on
exactly this posture: a sealed claim must not rest on another writer's unverified metadata. Reading
`null_count` to decide what a column *is* would put a producer's optional statistic into the identity of
every product ingested from its files.

## What would make it acceptable

Gated on **proof of agreement**, not on trust:

1. Use the footer path only when every column chunk in the file carries statistics — a single missing one
   falls back to the data pass.
2. A gate that, for every fixture in the ingest corpus, ingests **both ways** and asserts an identical
   `content_hash`. That makes the fast path a verified optimisation rather than an assumption.
3. Consider spot-verifying: take the footer's answer, and if it says "no nulls anywhere" for a column,
   confirm it on the data pass that pass 2 performs anyway — a wrong `null_count` then fails loudly
   instead of sealing quietly. This may make the optimisation pointless, which is itself worth knowing.

Worth measuring before building: how much of a multi-GB Parquet ingest's wall clock pass 1 actually is. If
the decode in pass 2 dominates, the fast path buys little and the trust question is not worth opening.

Refs: #458

