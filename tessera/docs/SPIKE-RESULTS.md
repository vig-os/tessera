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

---

# Capstone: volume codec, chunk size, and the access-pattern split

## Volumes — container is irrelevant, codec + chunking is everything
- **Zarr ≡ HDF5** byte-for-byte at equal codec+chunking (CT 94.5 = 94.5 MB).
- **Wide codec sweep (lossless):** CT int16 — zstd-3 94.5, gzip 96.6, zstd-9 90.6, delta+zstd
  88.6, blosc-lz4 159.8, **pcodec 74.3 MB**. PET float32 — zstd 0.9, **pcodec 0.6**;
  bitround+zstd 0.7 (lossy, *worse*), zfp 3.7 (lossy, far worse). **pcodec wins both, losslessly.**
- **Chunk-shape sweep (Zarr+pcodec, CT):** slice(1,Y,X) 80.5 MB (axial 0.017s but coronal/
  sagittal 0.24–0.25s = 6–7× worse, anisotropic); cube 32³ 71.5 (reads ~0.12s, overhead);
  **cube 64³ 74.3 (reads ~0.03s isotropic, ROI 0.008s — sweet spot)**; 128³ 76.3; 256³ 81.4
  (amplification, 0.12–0.15s). → **64³ cubic** = smallest-within-4% + fastest balanced + best ROI.
- **Why flat/Morton-Vortex loses (171/163 MB vs 74):** un-chunked = one global encoding model
  (no per-region adaptivity) + 1D-x-only locality + read amplification. Chunking fixes all
  three — and "chunked Vortex" = Zarr. "Zarr-on-Vortex" reduces to "chunk grid + a good codec"
  = Zarr+pcodec.

## Tables — Zarr+pcodec matches size but loses access
events_3p (2.7M rows), columnar Zarr+pcodec vs Vortex:
| | size | projection | random take×500 |
|---|---:|---:|---:|
| Zarr+pcodec (chunk 65536) | **73.3 MB** | 0.023 s | 0.168 s |
| Zarr+pcodec (chunk 4096) | 73.7 MB | 0.190 s | 0.986 s |
| Vortex | 77.7 MB | **0.0076 s** | **0.0233 s** |

pcodec *equalises (beats) Vortex on table size*, but Vortex wins **random take 7–42×** +
projection 3× + filter pushdown + zero-copy Arrow→DuckDB. **Tables keep Vortex for the
*engine*, not compression.**

## The split (final)
- **Volumes** = bulk-decode local N-D regions → **Zarr/OME-Zarr · 64³ cubic · pcodec**.
- **Tables** = project · filter · random-access · join → **Vortex engine**.
- **pcodec** is the universal *codec* (shared plumbing); the *container/engine* is chosen by
  access pattern. zstd = decades-stable fallback codec.

## Layout
- Canonical: **single sealed `.tsra`** = STORED zip64 (central-dir index → cloud range-reads
  into one file; inseparable). Opt-in **exploded S3 prefix** (≈ OCI image-layout) for parallel
  write / CoW versioning. **OCI artifact** (zot/registry:2 on MinIO, or bare OCI-layout objects)
  for registry distribution. **RO-Crate** derived for discovery. One Merkle root across all forms.

---

# S13 (bit-exact roundtrip) + S15 (writer determinism) — the existential gates, PASSED

Run before first push (pcodec 0.3.6, vortex 0.75.0).

## S13 — lossless to the bit (medical/quantitative gate)
| case | pcodec | Vortex |
|---|---|---|
| CT int16 | ✓ bit-exact | — |
| PET float32 (SUV) | ✓ bit-exact | — |
| float edge (NaN/±inf/−0.0/denormal) | ✓ bit-exact | ✓ |
| int16 limits | ✓ bit-exact | ✓ |
| events_3p mixed columns | — | ✓ bit-exact |

pcodec (arrays) + Vortex (tables) are lossless to the byte incl. float corner cases — no ULP
drift on SUV, no HU clipping. Clears the clinical/regulatory gate for the codec choice.

## S15 — writer determinism (content_hash = identity REQUIRES this)
pcodec CT int16 · pcodec PET f32 · Vortex events → all **byte-identical across 3 runs** ⇒
same input → same bytes ⇒ reproducible `content_hash`/identity. PASS.

**Caveat / remaining S15:** proven *same-version, same-machine*. Cross-version is NOT guaranteed
(pcodec/Vortex pre-1.0 — a release could change bytes) and cross-arch (x86/ARM/macOS) is untested.
Hedge = pin codec versions in the manifest + ship vendored pure-Rust readers (RFC §8/§14).

# #143 — `.tsra` vs the bare substrate (cross-substrate comparison)

Tool: `cargo run -p tessera-io --example bench_compare --release`. `.tsra` = sealed zip64 (manifest
+ blake3) over the **same** bare codec blob; "bare" = vanilla Zarr+pcodec (arrays) / Vortex (tables)
— the substrate Tessera wraps. Absolute MB/s is dev-box (not the §D 88-core box); the portable
findings are the **ratios, container overhead %, and partial-read speedups**.

## Arrays — int16 CT-like, 64³ chunks
| n³ | raw | pcodec ratio | `.tsra` size tax | ROI sub-cube | Z-slice |
|---|---|---|---|---|---|
| 32³ | 0.06 MiB | 11.5× | +25.3% | 1.0× | 1.1× |
| 64³ | 0.50 MiB | 107× | +29.7% | 1.0× | 1.0× |
| 128³ | 4.0 MiB | 128× | +4.4% | 7.7× | 6.8× |
| 256³ | 32 MiB | 131× | +0.6% | 7.4× | 25.9× |

## Tables — u64 + 2×f32 listmode (Vortex)
| rows | raw | ratio | `.tsra` size tax |
|---|---|---|---|
| 10K | 0.15 MiB | 12.6× | +11.8% |
| 100K | 1.5 MiB | 19.8× | +1.9% |
| 1M | 15.3 MiB | 21.0× | +0.2% |

**Reading:**
- **Container tax amortizes to ~zero.** The manifest + zip central-dir is a fixed cost: 25–30% on a
  sub-MiB toy, **0.2–0.6% at realistic sizes** (≥4 MiB array / ≥100K-row table). `.tsra` is free at scale.
- **Write/read throughput ≈ bare** (within a few % — the pack/unzip step is cheap vs the codec).
- **Partial reads scale with chunking.** ROI sub-cube and slicing only beat a full decode once the
  volume spans multiple 64³ chunks: 1.0× at ≤64³ (one chunk), **7–8× (ROI) and up to 26× (slice)** at
  128³–256³. This is the chunk-granularity lever (S2) realized through the `.tsra` read path.
- **Net:** Tessera's FAIR envelope (content-addressing, self-description, range-readable container)
  costs essentially nothing over the raw codec at real data sizes, while adding addressable partial reads.

## #143 (cont.) — cross-ecosystem comparison: `.tsra` vs HDF5 / Zarr / NeXus / NIfTI / DICOM / Parquet / ROOT

Tool: `tessera/bench/ecosystems/` (`uv run python run.py`, pinned `taskset -c 10-39 nice -n19`, min-of-5,
warm reads). Same synthetic volume (int16 256³, 32 MiB) + table (u8+2×f4, 1M rows, 15 MiB) through every
format's real Python writer/reader, each bit-exact-asserted. Each format uses a sensible lossless codec.

### Volume (32 MiB int16, 64³ chunks where supported)
| format | codec | ratio | write MB/s | read MB/s | **z-slice MB/s** | content-addr/sealed |
|---|---|--:|--:|--:|--:|:--:|
| **Tessera .tsra** | pcodec/Vortex | 130× | 393 | 357 | **16246** | ✓ |
| HDF5 (h5py) | gzip-4 | 47× | 172 | 445 | 2611 | — |
| Zarr (zarr-python) | zstd-3 | 246× | 490 | 621 | 3285 | — |
| NeXus (h5py/NXdata) | gzip-4 | 47× | 170 | 449 | 2635 | — |
| NIfTI (nibabel) | gzip | 18× | 117 | 452 | 146 | — |
| DICOM (pydicom) | uncompressed | 1.0× | 472 | 1043 | 1015 | — |

### Table (15 MiB, u8 + 2×f4, 1M rows)
| format | codec | ratio | write MB/s | read MB/s | **column MB/s** | content-addr/sealed |
|---|---|--:|--:|--:|--:|:--:|
| **Tessera .tsra** | Vortex | **21×** | 114 | **1180** | **4493** | ✓ |
| HDF5 (h5py) | gzip-4 | 10× | 101 | 468 | 2765 | — |
| Zarr (zarr-python) | zstd-3 | 15× | 246 | 489 | 1505 | — |
| NeXus (h5py) | gzip-4 | 10× | 101 | 466 | 2818 | — |
| Parquet (pyarrow) | zstd | 12× | 170 | 657 | 2344 | — |
| ROOT (uproot/TTree) | zstd-3 | 15× | 334 | 389 | 2343 | — |

SWMR / concurrent-reader: HDF5, Zarr, NeXus (yes). Tessera = immutable-sealed (versioned CoW, not live append).

### Reading (ALOCA)
- **Compression — Tessera is competitive, and wins on *real* data.** On this **smooth synthetic** gradient,
  zstd over-compresses (Zarr 246×) and beats pcodec (Tessera 130×) on the volume. **Caveat that flips the
  ranking:** on REAL noisy CT/PET the codec spike measured **pcodec −21% CT / −33% PET vs zstd** — the
  synthetic favors entropy coders, real medical data favors pcodec. On the table, Tessera (Vortex, **21×**)
  beats every columnar format incl. Parquet (12×) and ROOT (15×).
- **Partial reads = Tessera's structural win.** Volume z-slice: **~16 GB/s effective, 5–6× HDF5/Zarr/NeXus,
  ~110× NIfTI** — the chunked addressable read the others can't match (NIfTI's gzip stream forces a near-full
  decompress: 146 MB/s). 
- **Full-volume decode is Tessera's honest cost.** 357 MB/s — mid-pack (DICOM's raw memcpy is 1043); pcodec
  decode is heavier than gzip/zstd. The tradeoff buys 130× compression + addressable slices.
- **Column projection — now the fastest (#212, landed).** Originally Tessera's column read ≈ full read
  (1336) because the binding read the whole block. Wiring Vortex projection (`decode_column` +
  `read_table_column`) took it to **4493 MB/s — fastest of every format** here (HDF5 2970, ROOT 2637,
  Parquet 2464). So Tessera leads table compression (21×), full read (1180), AND column read (4493).
- **The envelope is unique.** Tessera is the **only** format here that is content-addressed + sealed +
  self-describing FAIR, and the **only** one that holds volume *and* table in one product with one API.
  HDF5/Zarr/NeXus are bare containers (no identity/integrity); NIfTI/DICOM are volume-only; Parquet/ROOT
  are table-only. Tessera's overhead for that envelope is ≪1% at these sizes (see the `.tsra`-vs-bare run).

### Real data — full-body CT (the synthetic ranking flips)
Same harness, `--real-dicom <series>` on a real 176-slice onco attenuation-CT (176×512×512 int16,
**88 MiB**; raw stored values, no rescale). Patient arrays are never committed — only these aggregate
numbers. Volume only (the table stays synthetic).

| format | codec | ratio | size MiB | write | read | z-slice |
|---|---|--:|--:|--:|--:|--:|
| **Tessera .tsra** | pcodec | **3.8×** | **23.1** | 239 | 284 | **3460** |
| HDF5 | gzip-4 | 3.3× | 26.5 | 59 | 201 | 710 |
| Zarr | zstd-3 | 3.3× | 27.0 | 428 | 585 | 2390 |
| NeXus | gzip-4 | 3.3× | 26.5 | 46 | 201 | 715 |
| NIfTI | gzip | 3.0× | 29.4 | 45 | 179 | 68 |
| DICOM | uncompressed | 1.0× | 88.0 | 336 | 651 | 630 |

**The flip:** on the smooth synthetic, zstd over-compressed (Zarr 246× vs Tessera-pcodec 130×). On
**real noisy CT, that inverts** — compression collapses to 3–4× for all, and **pcodec is now the
smallest (3.8×, 23.1 MiB)**, beating zstd/gzip — exactly the codec spike's −21% CT. So the synthetic
compression lead was a gradient artifact; real medical data favors pcodec, which is why it's the
default. Tessera also keeps the **fastest slice read (3460 MB/s, ~5× HDF5/NeXus, ~50× NIfTI)** and
its self-describing + content-addressed + sealed envelope, at the smallest size on disk.
(Full-volume decode 284 MB/s stays the honest pcodec cost — Zarr's zstd decodes faster at 585.)

# #203 — streaming write engine (bounded memory, parallel encode)

`StreamWriter` (`crates/tessera-io/src/stream.rs`) is the DAQ-facing front of `WriteSession`:
producer `push` → bounded `sync_channel` (backpressure = RAM cap) → N encode worker threads (parallel
pcodec/Vortex) → an **ordered committer** (reorder-buffers by sequence, calls `append_block` in push
order) → `WriteSession` (durable fragment + journal commit + hash-on-write Merkle) → `finish` seals.
Std-only (no rayon/tokio dep). Tool: `cargo run -p tessera-io --example stream_write --release`,
pinned `taskset -c 10-39 nice -n19`, 256 × 64³ int16 blocks (128 MiB raw).

| mode | wall s | blocks/s | MB/s | speedup |
|---|--:|--:|--:|--:|
| synchronous (encode on hot path) | 3.67 | 70 | 37 | 1.00× |
| stream w=1 | 2.28 | 112 | 59 | 1.61× |
| stream w=2 | 1.19 | 216 | 113 | 3.09× |
| stream w=4 | 0.60 | 427 | 224 | **6.13×** |
| stream w=8 | 0.59 | 432 | 227 | 6.20× |

### Reading (ALOCA)
- **6.2× throughput over synchronous**, saturating ~4 workers (encode-bound on this box). Even 1 worker
  gives 1.6× — encode now overlaps the committer's fsync+journal instead of blocking the producer.
- **Bounded RAM proven:** cap=2 with 4 workers completed (0.57 s) — peak in-flight ≤ ~cap blocks
  (≈1 MiB), not the full 128 MiB. Backpressure (`push` blocks on a full channel) holds memory flat no
  matter how fast the producer runs — the DAQ requirement.
- **Byte-identical to batch:** the committer commits in **push order**, so the running Merkle and the
  sealed `id`/`content_hash`/`manifest_hash` match a batch writer exactly (test
  `stream::tests::streamed_equals_batch_for_many_blocks`). Streaming changes *how* the bytes are
  produced, never *which* bytes — so it does NOT touch the v1.0 frozen format or the determinism gate.
- **Crash-safety inherited:** each committed block is a durable fragment + journal line; `recover`
  replays to the last watermark (existing `WriteSession` guarantees, unchanged).
- **Out of scope (ADR-0026):** sub-block chunked compaction for a single >RAM block — the only part
  that would change bytes (and the path that unifies >RAM ingest + live per-chunk Merkle). The multi-
  block streaming + live block-level Merkle land here; the huge-single-block case rides the same engine.

# #215 — array fold: pyramid + projection are one monoid-fold-over-axes (ADR-0028)

Claim: an array's multiscale pyramid AND its projections (MIP/MPR) are the *same* operation — a
mergeable monoid folded over a chosen subset of the chunk-grid axes to a chosen depth — so they fall
out of one per-chunk pass (the streaming committer already touches each chunk to encode/hash).

Spike (`/tmp/array_fold_spike.py`, real 128×512×512 CT, 64 MiB, 128 × 64³ chunks): one traversal of the
64³ grid, folding with `max`, accumulated **both**:
- **pyramid L1** = local 2³ max-pool → (64,256,256), and
- **MIP-z** = full-z max → (512,512),

**both bit-exact** vs `vol.reshape(...).max((1,3,5))` and `vol.max(0)`. So `max` folded *locally 2×* =
the OME-Zarr pyramid level, and `max` folded *fully along z* = the MIP — one monoid, parameterized by
`(axes, depth)`. mean/avg-projection are the same with `sum`+`count` (float canonical-reduction guard).
The fold is a SIMD `max`-reduction, ≪ the per-chunk pcodec encode + blake3 → ~free in the committer.

**Conclusion:** the factory generalizes from tables (1-D, axis=rows) to arrays (N-D): pyramids,
projections, profiles, and scalar stats are one `(monoid, axes, depth)` abstraction, materialized live
in the same fold that builds the integrity tree.

# #221-A — sparse representation: dense (zarr+pcodec) vs COO (Vortex), crossover

Harness: `cargo run -p tessera-io --example spike_sparse --release` (deterministic splitmix64 masks).
128³ int16 volume (4 MiB raw), occupancy 0.01 %→50 %, structure {scatter · banded · block}. Two COO
forms tested so the comparison isn't a strawman: **3c** = `(z,y,x,v)` columns; **lin** = single ascending
linear-index `(idx,v)` (the *fair*, best-compressible coordinate). On-disk KiB + relative materialize.

| structure | occ | nnz | dense | coo-3c | coo-lin | winner | mat × (COO÷dense) |
|---|--:|--:|--:|--:|--:|---|--:|
| scatter | 0.01 % | 225 | **2.8** | 8.1 | 4.7 | dense | 0.05× |
| scatter | 1 % | 21 250 | **50.8** | 71.6 | 87.6 | dense | 0.05× |
| scatter | 10 % | 210 127 | **401.5** | 521.0 | 828.3 | dense | 0.12× |
| scatter | 50 % | 1 048 572 | **1636** | 2371 | 3884 | dense | 0.89× |
| banded  | 1 % | 16 384 | **3.0** | 41.4 | 25.5 | dense | 0.10× |
| banded  | 50 % | 1 048 576 | **21.7** | 2321 | 1413 | dense | 0.73× |
| block   | 1 % | 21 952 | **8.7** | 50.6 | 86.1 | dense | 0.12× |
| block   | 50 % | 1 061 208 | **118** | 2387 | 3890 | dense | 1.06× |

Sizes are exactly deterministic (splitmix masks); `mat ×` is best-of-15 decode (timing is noisy — read
the low-occupancy rows as the robust signal, the ~parity high-occupancy cells as "no decode advantage").
**Caveat (banded):** the slice count is `clamp(1, n)`, so every banded occ < 1/n (here < 0.78 %) collapses
to the *same* 1-slice datapoint (0.78 % nnz) — the three banded sub-1 % rows in the raw run are one
measurement, not three (the run's `nnz%` column shows this).

**Result — dense+pcodec wins on-disk at EVERY occupancy and structure, even 0.01 % scatter.** pcodec's
zero-run compression + the tiny dynamic range of coordinates means neither COO form ever wins on bytes at
storable grid sizes. Block/banded confirm ADR-0031 §1 emphatically (dense + stat-prune crushes clustered
sparsity: 21 KiB for a 50 %-banded 4 MiB grid). So **there is no on-disk occupancy crossover in COO's
favour** at materializable scales — the ADR-0031 §5 "occupancy threshold" does not exist as posited.

**But COO's genuine win is read-RAM / latency, not disk:** `mat ×` is **0.05–0.15× at low occupancy**
(COO decodes 7–20× faster, RAM ∝ nnz, not grid volume). And the *forced* COO regime is **an ambient
dense grid too large to materialize** (high-D sparse histograms — e.g. a 6-D 10¹² -cell grid with 10⁶
nonzeros) where dense is not an option at all. That regime is asserted by construction, **not benched**
(you cannot allocate the dense grid — which is the point).

**Conclusion → ADR-0031 reframe:** replace "dense-vs-COO by an occupancy threshold" with a **decision
rule**: *dense+pcodec by default whenever the dense grid is materializable* (≈ all imaging masks/volumes
— it wins on disk AND block-prune handles clusters); *COO only when (a) the ambient grid is unstorable
(high-D histograms — dense impossible), or (b) the access pattern is selective point-reads where
nnz-bounded RAM/latency matters.* The lever is materializability + access pattern, not occupancy %.

# #221-B — chunk-index leaf granularity: index overhead vs pruning selectivity

Harness: `cargo run -p tessera-io --example spike_chunk_index --release` (uses the real
`tessera_core::chunk_index`). 1,048,576 i64 (8 MiB), ranged query = ~1 % of the value range. Index size =
conservative `(32-byte digest + 40-byte stat)·nchunks` (the real index is a compressed Vortex column, so
this over-states). `scan %` = rows in non-pruned chunks ÷ total (lower = better).

| pattern | leaf | nchunks | index % | scan % |
|---|--:|--:|--:|--:|
| sorted/clustered | 1024 | 1024 | 0.879 % | 1.1 % |
| sorted/clustered | 4096 | 256 | 0.220 % | 1.2 % |
| sorted/clustered | 16384 | 64 | 0.055 % | 3.1 % |
| sorted/clustered | 65536 | 16 | 0.014 % | 6.25 % |
| sorted/clustered | 262144 | 4 | 0.003 % | 25 % |
| **random** (any) | — | — | (same) | **100 %** |

**Findings:**
1. **Index overhead is negligible at every granularity** — <0.9 % of payload even at 1024-row leaves,
   0.014 % at 65536. So overhead is **not** the binding constraint; pruning effectiveness is.
2. **Pruning knee ≈ 4096–16384 rows** for locality-bearing columns: scan% falls 6.25 % → 3.1 % → 1.2 %
   as leaves shrink 65536 → 16384 → 4096, approaching the query selectivity (~1 %). Below ~4096 the
   gain flattens (4096→1024 only 1.17 %→1.07 %) while index % climbs — diminishing returns. There is no
   single sharp inflection: **16384** is the *conservative* default (halves scan vs 65536 at 0.055 %
   index); **4096** is equally defensible (near-ideal 1.2 % scan, still 0.22 % index) where pruning
   matters more than index size. (Sorted and clustered track within ~0.1 pt — both columns have locality.)
3. **Random / non-local columns can't be pruned at any granularity** (100 % scan) — min/max spans the
   global range, so the stats add nothing. Emit a **hash-only** index for such columns (integrity without
   the stat cost); detect via the column's own min/max-vs-global spread at encode time.

**→ Recommendation (ADR-0028 §3 / ADR-0027):** the chunk-index leaf granularity should be **decoupled
from the table payload's `ROWS_PER_GROUP` (2¹⁶)** — a finer index leaf of **~2¹⁴ (16384)** roughly halves
rows-scanned (6.25 % → 3.1 %) at trivial index cost (0.055 %), and is the recommended default for
sortable/clustered columns. Index leaves are an *index* concern, free to differ from the byte-payload
row-group size (which is frozen at 2¹⁶ for format compat). Hash-only fallback for non-prunable columns.
