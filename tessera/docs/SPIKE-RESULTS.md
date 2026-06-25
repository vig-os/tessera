# Spike results — table-backend decision

Real DUPLET data, cold cache (`posix_fadvise`), best-of-N. Caveat: just-written-file "cold"
reads are warm-ish (dirty pages survive `DONTNEED`) — treat absolute I/O as decode/throughput;
sizes and relative ordering are robust.

## Decision: **Vortex is the table-block backend.** zarrs is the array-block backend.

Vortex (Rust + Arrow-native; FastLanes bit-pack / ALP floats / FSST strings / FoR — addressable
encodings) won every measured axis vs Parquet and Lance. It slots *under* the Tessera spine, so
the Merkle tree stays **integrity-only**, not a random-access index. Lance is not needed.

## S0 — events_3p (2.7M rows, mixed dtypes)
| | Size | Full | Proj lt | Random take ×500 |
|---|---:|---:|---:|---:|
| Parquet zstd | 98.9 MB | 0.057 | 0.021 | n/a (no seek) |
| Lance | 92.2 MB | 0.063 | 0.021 | 0.032 s |
| **Vortex** | **77.7 MB** | ~0 | **0.012** | **0.0005 s** (confirmed under forced materialization) |

## S1/S2 — Parquet row-group sweep (can't tune Parquet into random-access)
| row-group | Size | take×500 | — 2.7M(1grp) 89.6MB/22.4s → 1k 129MB/0.30s. |
|---|---|---|
Floor ~0.30 s (600× Vortex, 10× Lance) **and** +44% size. Granularity drives take (S2 confirmed)
but addressable encodings are a stronger lever Parquet structurally lacks.

## S4 — scale, coins_2p 100M rows (uint16×5)
Parquet 739 / Lance 734 / **Vortex 725 MB**. Random take: Lance **0.904 s**, **Vortex 0.0003 s**.
Vortex take stays flat (O(1)) 2.7M→100M; Lance degrades ~28×. Size gap narrows on high-entropy
uint16 (~2%) vs structured events (~21%).

## S7 — DuckDB interop (lakehouse-for-free)
DuckDB over Parquet (native pushdown) 0.065 s; over **Vortex via zero-copy Arrow** 0.035 s —
identical result. Vortex storage keeps the full DuckDB/Polars/Spark ecosystem.

## S8 — mmap uncompressed Arrow-IPC (triangle's 3rd corner)
102.5 MB (1.3× > Vortex), full read 0.0006 s (true zero-copy-from-disk) but random take 0.226 s
(450× > Vortex). Not worth a flavor; Vortex dominates.

## S11 — pure-float table (7× f4) — ALP
**Vortex 60.7 MB** vs Lance 74.8 vs Parquet 78.7 (byte_stream_split didn't help) = **23% smaller**.
Random take 0.0004 s. Vortex's biggest size win is on floats.

## S10 — vectorized filter pushdown
`count + sum(lt) WHERE en0∈[450,550]`: Vortex pushdown 0.033 s vs materialize-then-filter
0.055 s (**1.7×**, results match). Modest here (82% selectivity); selective predicates skip more.
Compute axis works via pyarrow.dataset Expression in the scanner.

## Build/infra spikes (not benchmark-runnable here)
- **S3 Merkle-chunk index** — scoped down: Vortex owns random access, so Merkle is integrity-only
  (verified chunk addressing + pruning stats). Design in RFC §7; prototype pending.
- **S5 zarrs array backend** — perf pre-validated by the earlier Zarr-vs-HDF5 bench (sharded Zarr
  competitive/faster, codec-dominated); remaining work is the Rust `write_zarr` impl.
- **S6 object-store range-read** — needs MinIO/S3 infra; pending.
