---
type: issue
state: open
created: 2026-06-25T11:39:44Z
updated: 2026-09-28T17:59:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/203
comments: 3
labels: priority:high, epic, area:io
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:06.337Z
---

# [Issue 203]: [[P3][epic] tessera-io streaming write engine](https://github.com/vig-os/tessera/issues/203)

**Phase P3 — epic.** ROADMAP: tessera/docs/ROADMAP.md

- S5 zarrs array backend (64³ cubic + pcodec).
- S17 streaming write: bounded RAM ring → rayon encode pool → durable fragment commits + incremental Merkle + registry watermark (M/E/C); spill on burst; **never encode on the DAQ hot path**.
- S3 chunk-Merkle integrity tree (hash-on-write).
- container writer; observability (`tracing` on watermarks).

**Done-gate:** acq→sealed roundtrip; crash-recovery resumes to last committed watermark; §D perf-SLA met. (FEATURE-MATRIX §E.)
---

# [Comment #1]() by [gerchowl]()

_Posted on June 25, 2026 at 10:19 PM_

**S5 array backend landed** (commits `4d9c86c`, `f75389f`):

- `tessera-io::array` — real Zarr v3 + **pcodec** encode/decode for the 8 pcodec-native dtypes (i16/i32/i64/u16/u32/u64/f32/f64), plus `decode_subset` (ROI / orthogonal slab — only intersecting chunks).
- Block payload = one **deterministic** serialized Zarr store (sorted-key framing); digest over the encoded bytes via `add_block_ref`.
- **Bit-exact** incl float NaN/±inf/−0.0/denormal (the S13 clinical gate, now in Rust); **writer-deterministic** (encode-twice → byte-identical).
- Conformance corpus array fixtures now carry real payloads; goldens regenerated; repack byte-identical. ADR-0023 + SPEC §5a written.
- `nix flake check` green (39 tests). `ArraySpec` default codec → pcodec.

Remaining under this epic: **Vortex table backend** (next), then the streaming write engine (fragment-append, hash-on-write incremental Merkle, crash-recovery).

---

# [Comment #2]() by [gerchowl]()

_Posted on June 25, 2026 at 11:26 PM_

**Vortex table backend landed** (commits `b2041e4`, `7fdd538`):

- `tessera-io::table` — real Vortex columnar encode/decode for 10 numpy dtypes (`i1/i2/i4/i8`, `u1/u2/u4/u8`, `f4/f8`); `CurrentThreadRuntime`+session (no tokio); digest over encoded bytes via `add_block_ref`.
- **Conformance corpus is now fully real** — both table fixtures (`listmode_events`, `multiblock_study` ROI) carry real Vortex payloads, no spec-JSON stubs. `listmode` rows shrunk 1e6→4096 (a conformance fixture tests determinism, not scale).
- **Load-bearing determinism fix**: Vortex's default compressor chooses **ALP** for float columns, and ALP's float-exponent search is build-profile-sensitive (opt-level/FMA) → different bytes across builds → broke content-addressing (caught by the hermetic conformance gate). Fixed by excluding ALP/ALPRD schemes (`BtrBlocksCompressorBuilder::exclude_schemes`); floats fall back to deterministic flat/Pco. Verified dev==release==hermetic. (ADR-0024, SPEC §5b.)
- `flake.nix` provides libclang+libstdc++ for the Vortex hermetic build. `nix flake check` green (43 tests, cross-environment conformance).

Remaining under this epic: the streaming write engine (fragment-append, hash-on-write incremental Merkle, crash-recovery to watermark).

---

# [Comment #3]() by [gerchowl]()

_Posted on June 26, 2026 at 12:11 PM_

**Advanced in f5cabff** — the bounded-memory, parallel-encode streaming core landed.

`StreamWriter` (tessera-io::stream): push → bounded sync_channel (backpressure = RAM cap) → N encode worker threads → ordered committer (reorder by seq → `append_block` in push order) → `WriteSession` → `finish` seals. std-only, no new deps.

- **Byte-identical to batch** (commits in push order → same Merkle/seal) → does NOT touch the v1.0 frozen format or determinism gate.
- **Bounded RAM** under burst (full channel blocks push; peak in-flight ≤ ~capacity blocks).
- **Bench:** 6.2× throughput vs synchronous encode-on-hot-path (saturates ~4 workers), cap=2 holds RAM flat. (SPIKE-RESULTS #203, `examples/stream_write`.)

Done here: multi-block streaming + live block-level hash-on-write Merkle (FEATURE-MATRIX §E rows flipped to ✓).

**Remaining (kept open):** sub-block chunked compaction for a single >RAM block — the only byte-changing part, where >RAM file ingest and live *per-chunk* Merkle converge. Tracked in **ADR-0026**; gated on the cross-env determinism re-validation (#198) since it regenerates table goldens.

