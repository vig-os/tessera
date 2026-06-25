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
- Canonical: **single sealed `.tessera`** = STORED zip64 (central-dir index → cloud range-reads
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
