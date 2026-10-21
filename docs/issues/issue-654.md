---
type: issue
state: open
created: 2026-10-21T00:01:03Z
updated: 2026-10-21T00:01:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/654
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-21T07:01:44.897Z
---

# [Issue 654]: [feat(query): streaming DataFusion TableProvider over table blocks (projection + filter pushdown, chunk-stat pruning, bounded memory)](https://github.com/vig-os/tessera/issues/654)

## Problem
`tessera sql` (#251) materialises the whole table block into RAM before DataFusion runs (`tessera-cli/src/sql.rs`). A downstream consumer (MorePET list-mode, ~2.4×10⁹ rows per acquisition, joins of two row-aligned blocks) would need >50 GB RAM per query, and cohort queries over many `.tsra` are impossible.

## Ask
A library-level `TableProvider` (tessera-io or a feature-gated query crate, per ADR-0057) that:
- streams Vortex row groups as Arrow batches with bounded memory;
- pushes down projection and filters, and prunes chunks with the ADR-0059 chunk-index statistics;
- supports zero-copy positional zip of row-aligned blocks (same row count, possibly from a derived product) without a hash join;
- spans multiple `.tsra` / a Collection as one table (partitioned scan), local and over the object-store range reader (ADR-0034/#225).
Usable from Rust consumers, not only the CLI.

## Acceptance
Query over a >RAM table runs in bounded RSS; a selective predicate reads only matching chunks (instrumented); results equal the in-memory path.
