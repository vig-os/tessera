---
type: issue
state: open
created: 2026-10-21T00:01:03Z
updated: 2026-11-08T11:44:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/654
comments: 4
labels: feature, priority:high, effort:large, epic, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: 666, 667, 668, 669, 670, 671, 672, 673, 674, 675
synced: 2026-11-09T09:04:27.336Z
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


---

# [Comment #2]() by [gerchowl]()

_Posted on October 26, 2026 at 06:12 PM_

Owner decisions on ADR-0062's open questions (2026-10-26). ADR-0062 merged as Proposed in #661 (41de440); acceptance records these:

- **(a) Signed `aligned_to`:** not required in PR2 (same-producer zips, where the seal covers the claim). A signed alignment is **required** before any zip across producers ships, tied to #656/#566.
- **(b) Local read default:** `VerifyBlocks` without a spool; the spool is opt-in (remote or untrusted sources).
- **(c) Key mode under LIMIT / early stop:** today a row is released after only a neighbour check, so `[5,6][3,5]` zipped with a copy whose 5s are swapped returns a misaligned pair under `LIMIT 1` with no error. Decision: a key-mode zip **refuses** a pushed limit or early stop until a key-only pre-pass has verified strict increase over the whole scanned range; after that, LIMIT is allowed. This must be correct by construction, not a documented caveat. It goes into the ADR when it is accepted, and into #667's acceptance criteria.

---

# [Comment #3]() by [gerchowl]()

_Posted on October 29, 2026 at 06:20 AM_

ADR-0062 acceptance is up in #698. It records owner decisions (a), (b) and (c) of 2026-10-26 as normative text: (a) in §5 declared mode, (b) in §7, and (c) as §5's key-only pre-pass and release gate, with the `[5, 6] [3, 5]` counterexample. #667's acceptance criteria now include the (c) release-gate test. Open questions #672–#675 are listed as not blocking acceptance. Review found #697 (the non-key spool default), filed against #668.

---

# [Comment #4]() by [gerchowl]()

_Posted on November 8, 2026 at 11:44 AM_

**Blocker for epic #778 (scitadel-hub).** The corpus tables are 10⁷–10⁸ rows of string-heavy data, up to about 4.8×10⁸ for full OpenAlex works. Materialising into a `MemTable` (`tessera-cli/src/sql.rs:31`) is not an option at that size.

The consumer needs, in order:
1. #666: projection + filter pushdown with chunk pruning (year, topic and type filters are highly selective).
2. #668: multi-product / Collection scans over a quarterly base product plus daily delta products, **over the object store** (an S3-compatible object store, versitygw, gerchowl/g-fleet#1121).
3. Row ids that survive projection and filtering, so BM25 (#775) and vector (#769) top-k results can be joined back to their rows.

#666, #668 and stable row ids are the W1 gate in #778.

