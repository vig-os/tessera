---
type: issue
state: open
created: 2026-11-22T14:18:12Z
updated: 2026-11-22T14:18:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/952
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:28.575Z
---

# [Issue 952]: [feat(io): diff-ingest a full upstream generation against a sealed base — emit only the rows whose declared content changed, plus tombstones](https://github.com/vig-os/tessera/issues/952)

## Motivation

scitadel-hub (nerdmachines/scitadel-hub#89, S-015) keeps one full OpenAlex snapshot generation per quarter and wants one full generation plus deltas, not four full ones. #781 describes the *read and write* side of keyed deltas (upsert, tombstone, newest-row-wins, compaction). What is missing is how a delta gets **made** when the source does not publish one: upstream gives only a full new generation, and its change stamp is useless as a test of change.

Measured (the only listable release, 2026-10-14; partitions are by `updated_date`, so the records new or changed since a release are the partitions dated on or after it): **67.7 % of the jsonl bytes changed stamp in 21 days and 90.8 % in a quarter**, because upstream moves `updated_date` for any field, a citation count included. A stamp-based delta of a quarter therefore dedupes about 9 %.

## What exists

- #781: keyed delta products over a base, tombstones, compaction. It takes a delta as input.
- `tessera ingest table`: one source table into one product, no comparison with an earlier product.

## Ask

A diff-ingest, in the CLI and the Python API: given a sealed base `B` (or a collection) and a new full table `G` with the same key, write a delta product holding only
1. the rows of `G` whose key is absent from `B` (new),
2. the rows whose declared **content columns** differ from `B`'s row of the same key, compared by a row hash over those columns (the columns a consumer declares as the kept fields, not the stamp or the counters), and
3. a tombstone for every key of `B` absent from `G`,

plus the counts of each class in the manifest, so a run can state how much of a generation actually changed. The comparison should stream both inputs in key order (the blocks are sorted) and not hold either in memory.

## Acceptance

- A delta of `G` against `B` read through #781's logical read equals `G` on the declared content columns (property test over random edits, inserts and deletes).
- Identical `B` and `G` give an empty delta.
- The row hash is part of the sealed spec (which columns, which hash), so two runs give the same delta bytes.
- RSS stays bounded for tables larger than memory.

Related: #781, #906 (reclaiming superseded bases), #945 (kit's data engine index).

