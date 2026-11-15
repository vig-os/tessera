# ADR-0024 — Table block payload: a single deterministic Vortex file

Status: **Accepted** (2026-06-26; amended 2026-11-09, vortex 0.93 format event, #635) · Relates to:
ADR-0020 (identity/hashing), ADR-0022 (`.tsra` container), ADR-0023 (array payload). Implements the table half of ROADMAP
**P3 / S5** (`#203`).

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

## Amendment — vortex 0.93.0, a table-byte format event (2026-11-09, #635)

The codec moves from `vortex` 0.75 to **0.93.0**, pinned to `gerchowl/vortex@5c55cf7` (0.93.0 plus the
#468 fix, which upstream hasn't merged; the #472 fix shipped upstream in 0.89.0). This is a deliberate
**format event for table blocks**, landed on its own and not folded into another change.

- **Cause.** 0.93 writes each column's statistics as a `vortex.zoned` zone map instead of `vortex.stats`. The
  zone map has no `sum` and no `*_is_truncated` fields. The column **encodings are unchanged**: same scheme per
  column, same wire IDs. The vortex file-format version (footer `VERSION = 1`) is also unchanged.
- **Edition.** 0.93's writer refuses any wire ID outside the session's enabled editions. Tessera enables one
  frozen edition, `tessera_io::table::TABLE_EDITION = core2026.08.3`, named in the code so a vortex bump can't
  move it silently.
- **Codec set.** 0.75's codec set is kept: Pco for floats (through the compact preset; no zstd is compiled in).
  ALP and ALPRD stay excluded for the reason in §Determinism. Integer Pco, Delta and OnPair are also excluded,
  because 0.75 never used them and admitting them would be a second, unrelated byte change. Admitting a
  codec, zstd strings included (#628), is a later format event of its own.
- **Identity.** Re-ingesting or re-building the same input under 0.93 gives a **new `content_hash`** (and
  `manifest_hash`) for every product that holds a table block. Array, blob and table-free products are
  byte-identical. The corpus goldens were regenerated in the same change and every moved digest is listed in PR
  #788.
- **Compatibility.** Products written by 0.75 still open, read and verify. Their sealed bytes are unchanged and
  still hash to their recorded digests. `tests/fixtures/legacy_flat_events.tsra` (written by 0.75) stays frozen
  as that read-compatibility guard.
- **Size.** Table blocks get **5–10 % smaller** (corpus `listmode_events` 5060 → 4568 bytes, `multiblock_study`
  3388 → 3136 bytes), because the zone map is smaller. Column payloads are within ±0.2 % on every probe column.
- **Determinism.** dev and release write identical bytes on x86_64 and on aarch64, and both architectures
  compute the same new digests. The #468 guard now checks the cause (no persisted sortedness statistic on patch
  indices) rather than a length.
- **Not recorded.** No sealed field names the vortex version or the edition a table block was written
  with. The footer records only the wire IDs it uses. So "same input + same edition → same bytes" can be
  checked today only by also knowing the writer build. Whether to record the edition is an open design question
  (#635).
