---
type: issue
state: open
created: 2026-11-11T01:15:16Z
updated: 2026-11-11T01:15:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/807
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:38.606Z
---

# [Issue 807]: [feature(py): row-range, take and per-leaf reads of a logical table](https://github.com/vig-os/tessera/issues/807)

## Gap (from the #801 consumer gap analysis)

The Python `Reader` reads whole columns or whole logical tables (`table_dict`, `column`, `table_arrow`, `table`; `tessera/crates/tessera-py/python/tessera/__init__.py:123-152`). It has no row range, `take` or `locate`, and no block-pruned read, although Rust has them on `LogicalTableView` (`take`, `locate`, `column_blocks`, `select_blocks_overlapping`; `tessera/crates/tessera-io/src/multiblock.rs:99-323`).

geddf-analysis, a #803 reader, fans out over row ranges: `_chunk_row_ranges` picks `tm_ms`-aligned row ranges of `listmode_singles`, and each `multiprocessing` worker reads only its range (`geddf_analysis/process_sim_listmode.py:310`, `:1231`, MorePET/geddf-analysis at `d6cd785`). Porting it to tessera today means every worker materialises the whole table.

## Ask
Expose these on `Reader`:
- `rows(name, start, stop)` (logical row range, columns optional), and `take(name, indices)`;
- the per-leaf row counts (`block_rows`), so a caller can split by leaf;
- a stats-pruned select over the `.cidx`, if cheap.

Each needs a test and the `api_drift.py` coverage, and they should compose with #584 (zero-copy Arrow).

