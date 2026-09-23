# Spike — generic ingest: mapping foreign data into Tessera primitives (#386)

> **⚠ SUPERSEDED by [ADR-0056](../adr/0056-generic-ingest-normalise-vs-preserve.md)** (the decision).
> This is the exploration note that *seeded* the spike; the spike's parallel-review panel refined and in
> places overrode it — **where they differ, ADR-0056 wins.** Notably: ADR-0056 **rejects** the
> "lossy-map-with-warning" lane floated below (a warning doesn't travel with the seal, and sealing
> degraded values asserts they're the truth), replacing it with *lossless-with-a-transform-recorded-in-
> the-seal*; and it settles the open questions (spec-first dispatch, arrow→primitive determinism gate,
> the four+one additive `Column` fields, CSV/TIFF/auto-detect deferred). Kept only as the design trail.
>
> **Status (original):** design note for a `/spike`, resumable in fresh context — read "Current state",
> "Decisions", "Open questions", "Plan". (The Open questions are now answered in ADR-0056.)

## TL;DR

Tessera can already do two things with foreign data; #386 fills the gap between them:

- **`tessera ingest blob`** (ADR-0038) = the **"FAIR zip"**: wrap arbitrary/opaque bytes bit-faithfully in
  a sealed `.tsra`. You get identity · seal · provenance · versioning · signing · OCI/cloud distribution,
  but the payload is a **black box** (no per-shape codec, no column projection, no ROI reads, no
  field-level integrity, not queryable).
- **`tessera ingest <vendor>`** (ADR-0025) — DICOM / GE-HDF5 today — **normalises** the source *into
  Tessera primitives* (Vortex table blocks / Zarr+pcodec array blocks). You get everything blob gives
  **plus** compression, query/projection/ROI, and cross-arch-deterministic content-addressed bytes.

**#386 = extend the *normalising* path to the common generic formats** (Parquet, NumPy, NIfTI, TIFF, CSV)
so a user with their own data gets a *structured* `.tsra` in one command — not just vendor formats or an
opaque blob. It does **not** replace `blob`; blob stays the explicit fallback for anything un-normalisable.

## Current state (what exists — do not rebuild)

- **Primitives** the ingest must target:
  - **Table** = **flat, typed columns only** — the runtime `ColumnData` enum (`tessera-io/src/table.rs`):
    `I8 I16 I32 I64 U8 U16 U32 U64 F32 F64 Bool Utf8 Nullable`; the manifest's `Column`
    (`tessera-core/src/block/table.rs`) likewise carries only a string `dtype`, no nested escape. **No
    nested types** (no List / Struct / Map). Encoded to Vortex (btrblocks;
    ALP-excluded + Pco-registered for float determinism, #380/#384; grid-parallel decode #352/#385).
  - **Array** = dense **N-D** numeric grid → Zarr v3 + pcodec (64³ chunks, sharded ROI). Codec selectable
    (`pcodec` default / `zstd` / `auto`).
- **Ingest verbs today** (`tessera ingest …`): `dicom`, `dicom-series`, `ge-hdf5`, `blob`, and a
  declarative `--spec FILE.toml` multi-product path. All vendor-shaped except `blob`.
- **blob tier** (ADR-0038): `tessera ingest blob <in> <out> --name … --timestamp … [--media-type]` — the
  FAIR-zip; recoverable byte-identically via `tessera extract`.
- **Ingest engine** (`tessera-ingest`, ADR-0025/0035): format-tagged dispatch over closed backends →
  products/collections. New generic backends plug in here, not in a new subsystem.

## Decisions (locked — do not re-litigate in the spike)

1. **Two verbs, one ladder — not either/or.** `blob` = wrap-anything (FAIR-zip). `ingest table/array` =
   normalise the *structured* formats into primitives (the real value). #386 is the second; the first
   already ships and stays the explicit fallback.
2. **Route by the DATA's SHAPE, not the source format** — the whole premise of Tessera is that the
   primitive follows the shape (dense N-D → array, event/record table → table), independent of the
   container it arrived in.
3. **But never silently guess *semantics*.** The bytes cannot tell you whether a 2-D block is an image
   (→ array) or an N-row × M-col table (→ table). So:
   - **Default = honour the source's structural shape** (a columnar file → table; an N-D dense file →
     array).
   - **Explicit override** `--as table|array` for when the source misrepresents the data.
   - An **`analyze`/suggest** step may *flag* a likely mismatch but must **never auto-decide** the
     primitive from raw data.
4. **There is NO straight Parquet→Vortex (byte) mapping — and we don't want one.** Ingest is a **logical
   re-encode**: `source → Arrow/ndarray → Tessera columns/array → re-encode via the shape's primitive →
   seal`. Re-encoding is the *point*: Tessera imports the *logical values* and re-seals them under its own
   **deterministic** codecs (a byte-copy would import the source's non-deterministic encoding and defeat
   the seal). The flat-type constraint (see "Current state") is the boundary: nested/exotic
   source columns → flatten if trivial, else fall back to `blob`.
5. **The "previous format is misused" case is a FEATURE, not a bug to hide.** People dump a flattened
   volume into Parquet rows, or a table into an HDF5 2-D dataset. Ingest must not *perpetuate* the misuse
   silently; the `analyze`/`--as` mechanism is exactly where Tessera adds judgment at the door.
6. **Dispatch: spec-first, CLI verbs are thin wrappers.** A generic backend registers in the
   `tessera-ingest` engine the *same way the vendor ones do* — a `format = "parquet"` backend under the
   declarative `--spec` engine (ADR-0035; the GE listmode path is already spec-driven, not bespoke Rust).
   The new `tessera ingest table/array …` CLI verbs are thin front-ends that build a one-product spec and
   run it through that engine. So there is **one** ingest code path; the CLI is sugar, not a second
   subsystem. (Prevents the "new subcommand vs new spec backend" fork.)

## Format → default primitive (heuristic, always overridable with `--as`)

| Source | Default primitive | Notes / risks |
| --- | --- | --- |
| Parquet / Arrow / Feather | **table** (flat cols) | nested (list/struct/map) → flatten if trivial, else `blob`. Map logical types (see open Q2). |
| CSV / TSV | **table** | needs **type inference** (delimiter, header, per-column dtype, nulls) — the one genuinely hard corner (open Q4). |
| NumPy `.npy` / `.npz` | **array** | shape + dtype explicit → clean. `.npz` = multiple arrays → a collection. |
| NIfTI `.nii[.gz]` | **array** | + affine/world-frame → ADR-0030 spatial referencing (open Q6). |
| TIFF / OME-TIFF | **array** | multi-page/pyramidal → collection or levels. |
| anything else / un-normalisable | **`blob`** (FAIR-zip) | the honest fallback; `analyze` should say so explicitly. |

## Open questions (the spike MUST resolve before /land)

1. **`analyze` aggressiveness** — what heuristics flag a "misused" source? (e.g. a Parquet whose columns
   are all one dtype and index like a grid → "may be an array"; an HDF5 2-D dataset with a header row →
   "may be a table".) How loud, and is it opt-in?
2. **Arrow → Tessera type map** — exact coverage of the flat boundary. What maps (int/uint/float/bool/utf8,
   nullable)? What about decimals, timestamps/dates, dictionary/categorical, large_utf8, fixed-size lists?
   Enumerate: **maps cleanly / lossy-map-with-warning / reject-to-blob**.
3. **Nested Parquet** — flatten (how — dotted column names? explode lists?), reject-to-blob, or error with
   a clear message? Pick one default + an override.
4. **CSV type inference** — build it now or **defer**? Recommendation: **phase it later** — start with the
   self-describing formats (Parquet/NumPy/NIfTI) where dtypes are explicit and there's no guessing, so v1
   ships without an inference-heuristics rabbit hole.
5. **Schema attachment (⚠ Phase-1 blocker)** — which `ProductSchema` does a generic ingest stamp? A
   permissive builtin `table`/`array`? User-declarable via `--schema`? Fields' sensitivity tiers
   (ADR-0040) — default `public`? **This gates Phase 1**: shipping generic ingest without a decided
   schema/tier default silently leaks unclassified fields, so resolve it before, not after, Phase 1.
6. **NIfTI affine** → ADR-0030 referencing: how much of the world-frame do we carry on ingest v1?
7. **Determinism + decoder crate choice** — the re-encode must be cross-arch byte-reproducible. Tessera's
   own codecs already are (conformance gate); the risk is the *source decode* leaking arch-dependent
   values. This depends on the reader crate, so pick + pin them here: candidates are **arrow-rs**
   (`arrow`/`parquet` — Tessera already pulls Arrow via Vortex, so lowest new surface) for Parquet/Arrow,
   **ndarray-npy** for NumPy, **nifti** (nifti-rs) for NIfTI, **tiff** for TIFF. Add a parquet→tsra
   round-trip to the corpus/cross-arch gate.

## Plan (once the open questions are resolved → /land)

- **Phase 1 — self-describing formats, explicit verbs.** `tessera ingest table <parquet|arrow>` +
  `tessera ingest array <npy|nifti|tiff>` → primitives via `source → Arrow/ndarray → re-encode → seal`.
  `blob` fallback on unmappable columns; `--as table|array` override. New backends in `tessera-ingest`,
  dispatched like the vendor ones. Tests: value round-trip (`parquet→tsra→read == source`), nested→blob,
  cross-arch determinism.
- **Phase 2 — judgment + CSV.** `tessera ingest analyze <file>` (inspect + *suggest*, never decide;
  flag misuse), and CSV type inference.
- **Phase 3 — one-command UX.** `tessera ingest <file>` auto-detects format + shape → primitive, else
  prints "can't structure this — `tessera ingest blob` to preserve it as-is (FAIR envelope, opaque
  payload)."

## Relationships

Builds on **ADR-0025** (ingest model), **ADR-0035** (declarative spec engine), **ADR-0038** (blob /
FAIR-zip fallback), **ADR-0030** (spatial referencing for NIfTI), **ADR-0040** (sensitivity tiers — what
does a generic ingest tier its fields as?). The flat-`ColumnData` constraint is the load-bearing boundary
for the Parquet type-map. Likely lands its own ADR ("generic ingest & the normalise-vs-preserve ladder").
