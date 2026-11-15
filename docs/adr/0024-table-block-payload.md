# ADR-0024 — Table block payload: a single deterministic Vortex file

Status: **Accepted** (2026-06-26; amended 2026-11-09, vortex 0.93 format event, and 2026-11-15, the sealed
`TableSpec.edition`, #635) · Relates to:
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
- **Edition recorded.** As first written, no sealed field named the edition a table block was written
  with; the footer records only the wire IDs it uses. The owner decided (2026-11-11) to record it as a sealed
  `TableSpec` field, shipped in the same format event: see the next amendment.

## Amendment — the edition is a sealed `TableSpec` field (2026-11-15, #635)

Owner decision of 2026-11-11, option A of the three on #635 (A: a sealed `TableSpec` field; B: the
recipe bag; C: record nothing). It ships in the same change and the same format event as the vortex 0.93
bump above, so the corpus goldens move once.

- **Field.** `TableSpec.edition: Option<String>`, the Vortex edition id as Vortex prints it
  (`<family><year>.<month>.<version>`, today `"core2026.08.3"`). It is the frozen set of encoding wire IDs
  the writer was allowed to emit.
- **Who sets it.** The writer, never the caller. Every table block and partition leaf `tessera-io` seals
  goes through `table::sealed_spec`, which records `TABLE_EDITION`. A caller's spec leaves it `None`. A spec
  that names another edition is refused (`Error::Codec`; `push_table` and `open_table` refuse it before
  staging anything), because this writer can only write its own and recording another name over its bytes
  would make the field false. A caller that encodes with the public `table::encode` and builds its own
  `BlockRef` bypasses these paths and must set the field itself.
- **What it binds.** It sits in the manifest, so `manifest_hash` covers it. It is in no payload, so
  `content_hash` and the block digests don't change because of it. In the regenerated corpus, adding it moves
  `manifest_hash` of every table-bearing product and nothing else; the content side moved only through the
  0.93 stats layout above.
- **The check it enables.** "Same input + same edition → same bytes", read as a necessary condition. A
  product now says which Vortex codec contract its table bytes were written under. So when a re-encode or
  another product's table digest differs, the products alone tell whether the edition explains it: a different
  edition may legitimately give different bytes, and moving `TABLE_EDITION` is a format event of its own. The
  same edition is **not sufficient** for the same bytes. The edition bounds which encodings the writer may use,
  not which it picks. ALP, ALPRD, integer Pco, Delta and OnPair are all inside `core2026.08.3`, but tessera's
  compressor excludes them (§Codec set above). The pinned Vortex's compressor heuristics choose among the rest.
  Both are pinned by the writer build and change only as a format event. Whether to record them in the
  product too (a codec-profile id beside the edition) is the open question #869.
- **Absence.** A block without the key was sealed before this amendment, by the vortex 0.75 writer, which
  had no edition gate. Which encodings it could emit is known only from that writer build (ADR-0024 as
  first accepted). Such a block is valid: it reads and verifies, and its manifest bytes and hashes are
  unchanged, because `None` is omitted when serialized (never written as `null`), as `partition` is
  (ADR-0064 §2). The core test `pre_edition_spec_parses_without_edition_and_reserialises_byte_identically`
  pins the byte identity; `tests/fixtures/legacy_flat_events.tsra` (vortex 0.75) is the real pre-edition product
  that still opens, verifies under its pinned `manifest_hash`, and reads.
- **Reading.** A reader never needs the field to decode: Vortex decodes by the wire IDs in the file,
  whatever edition admitted them. Readers MUST accept a table block with or without it.
- **Schema convention.** Block specs are opaque to the core manifest schema (ADR-0020: `BlockRef.spec`
  is shape-specific) and are not in the product-schema registry (`tests/derived-docs/schema-reference.md`
  lists product schemas, not block specs), so no schema version or registry entry changes. The field
  follows the repo's additive-field convention for sealed specs, `#[serde(default,
  skip_serializing_if = ...)]`, as `Column::nullable`, `Column::referencing` and `TableSpec::partition`
  did. `tessera/docs/SPEC.md` §5b states it normatively.
