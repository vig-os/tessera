---
type: issue
state: open
created: 2026-10-21T00:01:03Z
updated: 2026-10-25T09:11:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/654
comments: 1
labels: feature, priority:high, effort:large, epic, area:io, adr
assignees: none
milestone: none
projects: none
parent: none
children: 666, 667, 668, 669, 670, 671, 672, 673, 674, 675
synced: 2026-10-26T07:24:26.254Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 09:11 AM_

**Triage (architect, 2026-10-25): accepted as an epic. ADR first, because it adds new public library API and a public DataFusion dependency.**

The ADR has to settle:
- **Home:** which crate it lives in. A feature-gated `tessera-query` crate, or a feature on `tessera-io`, under the ADR-0057 thin-waist rules.
- **DataFusion version policy:** a DataFusion type in our public API couples our semver to theirs.
- **Bounded-memory contract:** what is bounded, and by which knob.
- **Pruning:** how filter pushdown maps onto ADR-0059 chunk statistics, including NULL semantics (#627/#632).
- **Positional zip:** the row-alignment invariant it relies on, and how a mismatch is detected and refused rather than silently joined.
- **Multi-product scans:** partitioning over a Collection, and the object-store path (#225/ADR-0034).

Implementation is split after the ADR, in order:
1. A single-block streaming provider: projection, filter and chunk pruning, and a bounded-RSS test on a table larger than RAM.
2. Positional zip of row-aligned blocks.
3. A multi-`.tsra`/Collection partitioned scan, local first, then the object store.

The issue's acceptance criteria (bounded RSS, instrumented chunk skipping, equality with the in-memory path) carry into each sub-issue.

Order: the ADR is drafted in the next design wave, in parallel with #632/#655. #657 depends on this.


