# ADR-0024 — Table block payload: a single deterministic Vortex file

Status: **Accepted** (2026-06-26) · Relates to: ADR-0020 (identity/hashing), ADR-0022 (`.tsra`
container), ADR-0023 (array payload). Implements the table half of ROADMAP **P3 / S5** (`#203`).

## Context
Companion to ADR-0023. Where arrays are dense N-D (→ Zarr+pcodec), **tables are columnar** — listmode
events, spectra histograms, ROI summaries. The settled backend (FEATURE-MATRIX §B; spike S0/S4/S7/S10/
S11) is **Vortex**: smallest on disk, O(1) random `take`, filter-pushdown, and zero-copy Arrow→DuckDB.
Table blocks were spec-JSON placeholders; this ADR makes them real, satisfying the same two gates:
**bit-exact roundtrip** and **writer-determinism**.

## Decision
1. **Codec.** Table blocks are encoded as a single **Vortex file** (`vortex` 0.75, `vortex-file`),
   one self-contained blob — Vortex's file format already serializes the whole columnar structure
   (data + per-column statistics + a flatbuffer footer), so unlike Zarr there is no multi-key store to
   flatten. All encoding lives in `tessera-io::table`; `tessera-core` stays I/O-free.
2. **Columns.** A [`TableSpec`]'s columns use the fd5 numpy-style dtype codes (`i1/i2/i4/i8`,
   `u1/u2/u4/u8`, `f4/f8`); [`crate::table::ColumnData`] is the typed in-memory form. Each column
   becomes a Vortex primitive array, assembled into a `StructArray` in declared order.
3. **Sync over async.** Vortex's writer/reader are async; we drive them with a
   `CurrentThreadRuntime` (vortex-io, no tokio) installed on the session, so the codec presents a
   blocking `encode`/`decode` like the array backend.
4. **Digest.** `BlockRef.digest` is `blake3` over the Vortex file bytes (ADR-0020), supplied via
   `ProductBuilder::add_block_ref`.

## Determinism — and the ALP exclusion (load-bearing)
Vortex is deterministic *within* a build, but the default compressor was found **not**
deterministic *across* builds: its **ALP** float codec searches for a scaling exponent via float
arithmetic whose result depends on the build profile's float codegen (opt-level / FMA contraction).
Same float columns → different bytes under `dev` vs the hermetic crane profile — fatal for a
content-addressed format (a table's `content_hash` would depend on the compiler that wrote it). This
was caught by the conformance gate (the `listmode_events` fixture, which has `f4` columns, drifted
between the dev corpus and the hermetic test) and confirmed by a dev-vs-release reproduction; thread
count and `target-cpu` were ruled out first.

**Decision:** exclude the ALP / ALPRD schemes from the table compressor
(`BtrBlocksCompressorBuilder::exclude_schemes`). Floats then fall back to the deterministic
flat/Pco schemes; integer schemes (chosen by exact integer math) are unaffected. Verified: `dev` and
`release` now produce byte-identical payloads, and the conformance corpus is cross-environment stable.
The cost is a modest size regression on float-heavy tables — acceptable: the §D table gates are
random-take / projection *speed*, not size, and content-addressing determinism is non-negotiable.
Re-enabling ALP is a future option only behind a determinism guarantee.

Alternative considered — a bespoke per-column pcodec frame (reusing the array backend) — was rejected:
it would forfeit Vortex's random-take / pushdown / Arrow interop, reversing a settled, evidence-backed
architecture decision; excluding one scheme is far less invasive.

## Consequences
- The conformance corpus is now **fully real** (no spec-JSON stubs): the `listmode_events` and the
  `multiblock_study` ROI fixtures carry real Vortex payloads; goldens regenerated (`id` unchanged).
  The `listmode` fixture's row count was shrunk (1e6 → 4096) — a conformance fixture tests
  determinism/correctness, not scale (scale lives in the perf benches, #206).
- New dep tree (`vortex` 0.75, ~Arrow-based, 267 crates). A transitive dep (`custom-labels`) runs
  bindgen, so `flake.nix` now provides `libclang` + libstdc++ to the hermetic crane derivations.
  `cargo deny` advisories stay clean (only the pre-existing paste ignore).
- Cloud-side column pushdown / random-take over the `.tsra` range reader is a later layer (S6); today
  `decode` returns whole columns.
