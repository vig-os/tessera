---
type: issue
state: open
created: 2026-09-28T20:04:26Z
updated: 2026-09-29T06:53:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/458
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:30.282Z
---

# [Issue 458]: [stream generic table ingest instead of reading the whole file into memory](https://github.com/vig-os/tessera/issues/458)

#386's generic table lane (`tessera-ingest::parquet_table`) reads every row group into memory before sealing. The streaming machinery already exists and is proven — `tessera_io::pack_streaming` + `TableMultiBlockSink`, used by the GE-HDF5 lane for bounded-memory ingest of multi-GB acquisitions (ADR-0026, #224) — it is simply not wired to the generic path.

Why it was left out of #386: wiring a row-group-at-a-time ingest onto the multi-block writer is a measurable piece of work with its own determinism question (the block partition must follow `tessera_io::partition_blocks` so a streamed ingest is byte-identical to a batch one, exactly as `ge_hdf5`'s two paths are), and bundling it would have mixed a performance change into a feature.

Scope:

- feed `arrow_table::canonicalise_batches` row-group-at-a-time into the multi-block sink
- a test that a streamed generic ingest and a batch one produce the **same** `content_hash` (the `ge_hdf5` pair is the template)
- pick the threshold the same way `hdf-compound` does (`streaming = "auto"` + `stream_threshold`), so small files stay single-block and the corpus does not regenerate
- an RSS measurement on a multi-GB Parquet, like #224's DUPLET numbers

Until then, a very large Parquet should be split by the producer.
