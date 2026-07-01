# ADR-0045 — Units & quantities: UCUM codes + `(scale, offset)` transform, store-don't-compute

Status: **Proposed** (2026-07-01) · Tracks `#219` · Ratifies + pins the units-half of the
`(transform, unit, frame)` primitive from ADR-0032 (as-built) · Relates to ADR-0025 (ingest —
lossless native dtype), ADR-0028 (per-level referencing derived, not stored), ADR-0029
(feature-by-presence), ADR-0030 (`world_frame.unit`), and the fd5 field model
(`FieldSpec.unit`/`_vocabulary`/`_code`).

## Context

Tessera already carries units in three places:

- **`ArraySpec.unit`** — UCUM code of the (rescaled) sample values (`"HU"`, `"Bq/mL"`, `"1/cm"`).
- **`ArraySpec.rescale_slope` / `rescale_intercept`** — the `affine_1d` intensity transform
  (`physical = stored·slope + intercept`; DICOM RescaleSlope/Intercept, NIfTI `scl_slope/inter`).
- **`FieldSpec.unit` + `FieldSpec.vocabulary` + fd5 `Coded { _vocabulary, _code }`** — units on
  metadata fields, plus controlled-vocabulary escape for categorical/coded values (DICOM Modality,
  RadLex, SNOMED).

ADR-0032 unified space/time/intensity/parametric into one `(transform, unit, frame)` descriptor.
#219 asks the units-shaped questions specifically: **which unit code system**, **how is a stored
value related to its physical quantity**, and **who computes** dimensional conversions.

## Decision

### 1. Unit code system — **UCUM canonical**, `vocabulary` escape for non-UCUM domain codes

- **UCUM** (Unified Code for Units of Measure — the FHIR / medical-scientific standard) for all
  physical quantities: `mm`, `s`, `Bq/mL`, `keV`, `1/cm`, `1` (dimensionless). Case-sensitive as
  UCUM specifies.
- **`_vocabulary`/`_code` escape** (the existing fd5 `Coded` pattern used on `FieldSpec`) for
  domain codes that are not UCUM: DICOM (modalities, SOP classes), RadLex (anatomy), SNOMED
  (findings), LOINC (labs).
- **Dimension** (length / time / activity / …) is **derivable from the UCUM code** — the format
  itself does **not** implement a units engine; a downstream UCUM library (Rust `ucum` crate,
  Python `pint`) parses and dimension-checks.

The rule is *what* the code says, not *what it means computationally*. Store the code; let
downstream compute.

### 2. Quantity = stored value + unit + optional `(scale, offset)` transform

Every array is stored **lossless native dtype** (ADR-0025). Physical values are recovered on the
read path via the `affine_1d` case of ADR-0032's transform:

```
physical = stored · rescale_slope + rescale_intercept       (with unit = ArraySpec.unit)
```

- **CT** — store `i16` HU-encoded raw ints; `slope=1, intercept=-1024, unit="HU"`.
- **PET** — store `i16` or `u16` counts; `slope=<DICOM RescaleSlope>, intercept=0, unit="Bq/mL"`.
- **Identity** — `slope=1, intercept=0, unit="1"` for already-physical arrays.

`ArraySpec::to_physical(stored) → f64` and `ArraySpec::value_referencing()` (returning the projected
`Referenced::from_rescale(...)` shape from ADR-0032) implement the read-path recovery. Storage
stays raw; nothing is baked in.

### 3. Store-don't-compute (the identity rule)

- **Storage stays raw.** Do not pre-multiply slope/intercept into stored bytes: that loses
  precision (a lossless `i16` becomes a lossy `f32`), destroys the round-trip, and defeats writer
  determinism (float non-determinism at ingest).
- **Physical is derived at read.** `to_physical` is applied downstream; the format ships the code
  + transform, the reader (or an Arrow/numpy consumer) applies them.
- **No units engine in-format.** Conversion (mm→cm), dimensional analysis (adding `s` to `m` is an
  error), and simplification (`Bq/mL` vs `MBq/L`) are downstream. Tessera's job ends at delivering
  the code faithfully.
- **The transform is meaning-bearing.** `(scale, offset, unit)` change what the array *is*, so
  they live in the manifest under `manifest_hash`. Stored as `f64` (`scale`, `offset`) + canonical
  UCUM string (`unit`) → **deterministic** digest.

### 4. Coded (non-numeric) fields — `Coded { _vocabulary, _code }`

Categorical metadata fields (modality, laterality, contrast agent, anatomy) use the fd5 `Coded`
convention:

```json
{ "_vocabulary": "DICOM", "_code": "CT", "label": "Computed Tomography" }
```

`FieldSpec.vocabulary` names the expected vocabulary; the value carries the concrete
`{_vocabulary, _code[, label]}`. Same principle as §3 — store the code, let downstream resolve.

### 5. Derived physical quantities are **products**, not descriptors

A quantity that needs *other* data to produce (SUV ← activity + patient weight + dose + decay
correction; ADC ← diffusion fit) is **not** a `(scale, offset, unit)` transform. It is a **derived
product** (ADR-0028/0029 sidecar / derived block) with:

- its **inputs** recorded as `derived_from` edges (ADR-0033/0036),
- its **recipe** captured (ingest spec ADR-0035 or a derivation manifest),
- its **own** `unit` on the derived block's `ArraySpec` (e.g. `"g/mL"` for SUV_bw).

HU and Bq/mL are `affine_1d` descriptors (§2); SUV and ADC are products.

## Consequences

- **One descriptor family, one code system.** UCUM everywhere for physical quantities;
  `_vocabulary`/`_code` everywhere for categorical values. No bespoke per-modality unit fields.
- **Lossless storage preserved** — raw ints on disk, physical values on read; round-trip is exact,
  writer-determinism holds.
- **No format-owned units engine** — we don't ship a UCUM parser or a dimensional-analysis library;
  we hand off to the ecosystem. This is the same store-don't-compute stance as ADR-0031.
- **Deterministic identity** — `(slope, offset, unit)` are canonical `f64` + string, in the
  manifest, hashed. Reordering fields in serialization is prevented by JCS (ADR-0020).
- **Composable, feature-by-presence** — a bare-index array (no unit, no rescale) is valid: its
  `value_referencing_opt()` returns `None` (ADR-0029; ADR-0032 §1).

### What's built vs still a gap

- **Built:**
  - `ArraySpec.unit` (UCUM string), `rescale_slope`, `rescale_intercept`, `with_rescale`,
    `with_unit`, `to_physical`, `from_physical`, `value_referencing` / `value_referencing_opt`.
  - `WorldFrame.unit` (spatial UCUM, ADR-0030).
  - `FieldSpec.unit` + `FieldSpec.vocabulary` + `Coded { _vocabulary, _code, label }` in the
    product-schema registry.
  - Canonical unit list + `Referenced::unit_is_canonical()` in `tessera_core::referencing`.
- **Gap: no UCUM *validation* enforced at write.** `ArraySpec.unit` and `FieldSpec.unit` accept
  free-form strings; only the `CANONICAL_UNITS` allow-list is checked, and even that is a
  read-path helper, not an ingest gate. Real UCUM validation (parse + dimension) needs a UCUM
  library — deferred out-of-format per §3 but should surface as a `--strict-units` ingest lint.
- **Gap: coded-vocabulary validation.** `FieldSpec.vocabulary = "DICOM"` is not cross-checked
  against a DICOM dictionary at seal; a `--meta coded` lint should flag unknown `_code` values
  against the declared vocabulary (this is the review's `--meta coded` finding).
- **Gap: no `dimension` field.** Non-UCUM vocabulary codes may want an explicit `dimension` hint
  (`"length"`, `"time"`) since dimension can't be derived from the code. Add as optional on
  `FieldSpec` when a non-UCUM vocabulary shows up; UCUM cases stay implicit.

## Non-goals

- **Not** a UCUM parser / dimensional-analysis engine in the format.
- **Not** a units-conversion API on the read path — `to_physical` returns the value in
  `ArraySpec.unit` and stops there.
- **Not** re-encoding stored bytes into a "canonical" unit — mm stays mm, cm stays cm; the code
  disambiguates.
- **No** silent unit inference: an array without `unit` is a **bare-index / dimensionless** array,
  never guessed.

## References

- Tracks #219; ratifies the units half of ADR-0032.
- ADR-0025 (lossless native dtype), ADR-0028 (referencing derivation), ADR-0029
  (feature-by-presence), ADR-0030 (`world_frame.unit`), ADR-0031 (sparse — same store-don't-compute
  stance), ADR-0032 (`(transform, unit, frame)` primitive), ADR-0033 (derived products).
- Code: `tessera_core::block::array::ArraySpec` (`unit`/`rescale_slope`/`rescale_intercept`/
  `to_physical`/`value_referencing`), `tessera_core::schema::FieldSpec` (`unit`/`vocabulary`),
  `tessera_core::schema::Coded`, `tessera_core::referencing::{CANONICAL_UNITS, unit_is_canonical}`.
