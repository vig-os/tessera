# Performance & the write engine

## Budgets and performance ratchets

Set `TESSERA_MAX_THREADS` before startup to a positive decimal integer to size automatic worker pools.
It covers write defaults, balanced recommendations, read runtimes, grid decode and benchmark sweeps.
Without it, system parallelism is used (one worker if unavailable). Invalid values, including zero,
negative, overflowing and non-Unicode values, are logged to stderr and ignored. The first use caches the budget for the process; values above system parallelism are clamped
to it with one stderr note. Each pool gets the budget independently; explicit worker settings remain caller-controlled.
Array codecs and parallel blob hashing share a private Rayon pool capped by this budget. This library
never reconfigures the application's global Rayon pool; zarrs also receives the codec concurrency target
per operation. Failure to create the private pool returns an I/O error. Read runtimes retain a
`max(budget - 1, 1)` worker pool to leave room for the calling thread when possible.
Nix checks use `NIX_BUILD_CORES`; `scripts/ksb-test.sh` uses its `--threads` count.

Both wall-clock ratchets (grid decode and chunked HDF5 reads) require exactly `TESSERA_PERF_RATCHET=1`.
Otherwise they print `SKIPPED`; correctness and routing checks still run. Run timing assertions only on
reserved CPUs. For example, from a checkout with access to a reserved builder:

```bash
TESSERA_PERF_RATCHET=1 scripts/ksb-test.sh --threads 8 HEAD tessera-io 'test(grid_decode_perf_ratchet)'
```

The wrapper carries the environment opt-in through Nix, but cannot yet reserve the builder's CPUs.
Its fixed reporting-pod CPU request does not cover the test derivation, and reserving the pod with
`--exclusive-cpus` does not fix that. Builder reservation is tracked in
[#1030](https://github.com/vig-os/tessera/issues/1030); see CONTRIBUTING for both ratchet invocations.
Shared-builder timings are not performance evidence.

## The write engine

Vortex (and any columnar format) is footer-at-end: a writer that dies before close loses the *whole*
file. So Tessera never encodes a live acquisition straight into one sealed file. The `tessera-io` write
engine instead: a bounded RAM ring (backpressure) → a parallel encode pool → durable **fragment**
commits, each folding its blake3 into an **incremental Merkle** and advancing a registry watermark. A
crash is recoverable to the last committed fragment; the full column forms at compaction, and the product
seals at completion (ADR-0026).

This gives **constant-memory streaming above RAM**: a 294 M-row DUPLET acquisition sealed at **147 MiB
peak RSS** (vs 3.63 GiB single-block), throughput scaling ~3× with the encode pool — and `content_hash`
is worker-count-independent (the same bytes regardless of parallelism).

A Rust producer streaming one table larger than RAM writes it with `StreamWriter::push_table`, which stages
each block's input on disk and seals the table as the leaves `<table>/0000`, `<table>/0001`, … of one logical
table, with a heap bound that doesn't grow with the row count. Upgrading from `table_job_streaming` is covered in
[Migrating a streamed table to `push_table`](./migrating-to-push-table.md).

`tessera bench` measures this on *your* host, so you can size RAM/threads for your acquisition rate:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/bench.trycmd}}

## The SLA floors

`FEATURE-MATRIX.md` §D records regression floors (don't go below). Representative measured numbers:

| Metric | Floor | Measured |
|---|---|---|
| Volume size (CT) | ≤ 0.80× zstd | 74.3 MB (0.79×) — pcodec, lossless |
| 3-D ROI (32³ of 128³) | — | ~2.3× faster than full decode (cubic chunks) |
| Table random `take` ×500 | ≤ 30 ms | 23 ms (Vortex) |
| Encode (pcodec, /core) | ≥ 60 MB/s | 113 MiB/s (gated bench) |
| Seal / hash (blake3) | ≥ 4 GB/s | 6.0 GiB/s |

The machine-independent compression-ratio floor is CI-gated; wall-clock floors are measured on the
reference box. Any bench below its floor fails the build.
