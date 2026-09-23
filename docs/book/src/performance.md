# Performance & the write engine

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
