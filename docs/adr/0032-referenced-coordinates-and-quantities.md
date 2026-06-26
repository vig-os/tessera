# ADR-0032 — Referenced coordinates & quantities: one `(transform, unit, frame)` descriptor

Status: **Accepted** (2026-06-26, as-built) · Tracks `#219` (units/quantities) + `#220` (time) ·
**Generalises** ADR-0030 (spatial referencing = the coupled-ND instance) · Relates to ADR-0028 (derived
per-level transforms), ADR-0029 (composition · feature-by-presence · nature-not-rank), the fd5 field model
(`unit`/`_vocabulary`/`_code`).

**As-built** (every §§ + Status-note precondition has tested code, confirmed by a fresh-context audit):
§1 descriptor + §2 taxonomy = `tessera_core::referencing::{Referenced, Transform}` (`identity`/`affine_1d`/
`affine_nd`/`lookup`, `apply_scalar`/`invert_scalar`/`apply_point`). §3 UCUM + `_vocabulary` escape =
`Referenced.vocabulary` + pinned `CANONICAL_UNITS` + `unit_is_canonical`. §4 frame = pinned
`CANONICAL_FRAMES` + `:<detail>` suffixes (`epoch:`/`baseline:`/`atlas:`) + `frame_is_canonical` +
`with_epoch`. §5 instances = `from_rescale`/`from_world_frame` bridges + `time_regular`/`time_irregular`/
`time_ticks`. §6 time-split (wall-clock RFC 3339 in metadata vs elapsed `epoch`/`s` quantity, integer
ticks, PET `decay_correction_reference` on `dynamic_pet`). §7 manifest placement = `ArraySpec.axis_
referencing` + derived `value_referencing`/`spatial_referencing`, in `manifest_hash`, deterministic. SPEC
§5c documents it. The "Gate (future)" bullet is post-flip activation, not a precondition.

## Context
Three "physical meaning" needs were on the table as separate designs: **units/quantities** (#219),
**time** (#220), and the already-shipped **spatial referencing** (ADR-0030). They share one structure —
each maps a *stored* index/value to a *physical* one via a transform, tags it with a unit, and anchors it
to a named reference. Designing them three times would spawn three descriptors that drift. This ADR
**generalises them into one primitive** and makes space/time/intensity/parametric *instances* of it.

## Decision

### 1. The primitive — a **referenced axis/value descriptor** `(transform, unit, frame)`
Every array **axis** and the array's **value** (the pixels) may carry one optional descriptor:
```
referencing = {
  transform: <see §2>,     # stored index/value → physical coordinate/quantity
  unit:      <UCUM code>,   # §3
  frame:     <named ref>,   # §4 — the meaning of the origin/zero
}
```
It is **optional** → **feature-by-presence** (ADR-0029): no descriptor = index/raw space (a sinogram
axis, a unitless count). A tool needing physical values checks "is there a `referencing`?", never a type.

### 2. `transform` — one operator, rank & regularity by **nature** (ADR-0029 §6)
A small closed taxonomy; the case is chosen by the axis's *coupling* and *regularity*, not its position:
- **`identity`** — index *is* the coordinate (already-physical integer axis).
- **`affine_1d`** — `physical = scale·stored + offset`. The separable, regular case: a time axis, an
  intensity rescale (HU, Bq/mL), regular histogram bins, integer-tick→seconds.
- **`affine_nd`** — a coupled matrix (e.g. the spatial 3×4 voxel→world). Multiple index axes map into a
  multi-component frame with off-diagonal terms (oblique orientation). **This is ADR-0030's affine.**
- **`lookup`** — explicit per-index physical values. The irregular case: variable histogram edges,
  non-uniform b-values, irregular PET frame mid-times.

The key unification: **`affine_1d` (scale/offset) and `affine_nd` (the spatial affine) are the same
operator at different rank.** A scalar rescale is the 1-D diagonal case of the spatial matrix. So units,
time, and geometry stop being separate mechanisms — they pick a rank/regularity of *one* transform.

### 3. `unit` — UCUM code, dimension derivable, **store-don't-compute**
Units are **UCUM** codes (`mm`, `s`, `Bq/mL`, `keV`, `1` for dimensionless) — the FHIR/medical-scientific
standard; the **dimension** (length/time/activity/…) is *derivable* from the code. A `vocabulary` escape
(reusing the fd5 `_vocabulary`/`_code` pattern) carries non-UCUM domain codes. The format **stores the
code and never implements a units engine** — conversion/dimensional-checking is a downstream UCUM library
(same store-don't-compute principle as ADR-0031). Axis `unit` must agree with the descriptor's unit
(gate-checkable once Accepted).

### 4. `frame` — the named reference for the origin/zero
Generalises ADR-0030's `space`. It answers "what does coordinate 0 / value 0 *mean*":
- **spatial** → `space` = `patient | scanner | aligned | atlas:<id>` (+ `convention` = LPS canonical, ADR-0030).
- **temporal** → `epoch` = `unix | acquisition_start | injection | <named instant>`. An **absolute
  instant** = an elapsed quantity **+** its epoch.
- **intensity** → a named baseline (`hounsfield` water=0/air=−1000; `decay_corrected@<instant>` for PET).
- **parametric** → named or none.
A `frame` without a transform is just a label; a transform without a frame is a relative quantity. Both
optional, composable.

**Encoding (as-built).** A `frame` is a **bucket** from the pinned set `{physical, epoch, patient,
scanner, aligned}` (`CANONICAL_FRAMES`), optionally **refined** with a `:<detail>` suffix mirroring the
spatial `atlas:<id>` convention: `epoch:<instant>` (`unix`/`acquisition_start`/`injection`/…),
`baseline:<name>` (`hounsfield`, `decay_corrected@<instant>`), `atlas:<id>` (space). The bare bucket is
the minimal valid frame; the specific named instant rides the suffix (and, for PET, is **also** recorded
as the `decay_correction_reference` metadata field on the `dynamic_pet` schema, so a reader has it
without parsing the frame string). `Referenced::frame_is_canonical()` enforces this vocabulary; the
intensity bucket is spelled `physical` (a `baseline:` suffix names the specific zero).

### 5. Instances (the whole point — no bespoke per-domain fields)
| domain | transform | unit | frame |
|---|---|---|---|
| spatial geometry (ADR-0030) | `affine_nd` 3×4 | `mm` | `space` + convention |
| time axis, regular | `affine_1d` | `s` | `epoch` |
| time axis / event times, irregular | `lookup` | `s` | `epoch` |
| intensity (HU, activity) | `affine_1d` | UCUM | baseline |
| parametric (b-value, energy, bins) | `affine_1d` or `lookup` | UCUM | named/none |

Derived physical quantities that need *other* data (SUV ← activity + dose + weight + decay; ADC ← diffusion
fit) are **not** descriptors — they are **derived products** (a `transform`/sidecar with its recipe,
ADR-0028/0029), carrying their inputs + provenance. HU/Bq-rescale *is* a descriptor (just `affine_1d`);
SUV is a product.

### 6. Time specifics (absorbs #220)
Time = §1 applied to the temporal axis, with two kinds kept apart:
- **wall-clock absolute** (acquisition datetime, provenance) → **UTC RFC 3339** strings in metadata (human
  + audit; subject to leap seconds — fine for stamps, never for intervals).
- **elapsed/relative** (durations, frame offsets, event times) → a **quantity on a monotonic scale**
  (`affine_1d`/`lookup`, unit `s`, an `epoch` frame). **High-rate event timestamps** are **integer ticks
  + scale** (`tick→s`), not floats — lossless and compact, riding the columnar/COO machinery (ADR-0031).
PET **decay-correction reference** is a named instant (an `epoch`) recorded in metadata.

### 7. Identity, determinism, SSoT
- The descriptor is **meaning-bearing** (it changes what the array *is*) → it lives in the manifest under
  `manifest_hash`. Stored as `f64`/`int+scale`/canonical RFC 3339 → **deterministic** digest.
- **Lossless storage** preserved: pixels/indices stay raw; the physical transform is applied **downstream
  at read**, never baked into stored bytes (store-don't-compute).
- **SSoT with the pyramid** — per-level transforms are *derived* (ADR-0028/0030 §3), never stored
  per-level; the descriptor sits at level-0.

## Consequences
- **One descriptor, four domains** — units, time, intensity, and parametric axes stop being bespoke
  fields; spatial referencing (ADR-0030) is retroactively *the `affine_nd` instance* of this primitive
  (no rewrite — a generalisation relationship).
- **Rank-agnostic transform** — `affine_1d` and `affine_nd` are one operator; the codebase implements
  affine-apply once and reuses it for rescale and geometry (DRY/SOLID).
- **No units/time engine in-format** — UCUM codes + RFC 3339 + transforms are *stored*; conversion and
  dimensional analysis are downstream (store-don't-compute, consistent with ADR-0031).
- **Composable & optional** — feature-by-presence; a product carries exactly the descriptors it needs.
- **SPEC additions:** the `referencing` descriptor (transform taxonomy · UCUM unit · named frame); the
  instance table; the wall-clock-vs-elapsed time split; integer-tick event times; the identity/manifest
  placement. **Field-model touch-points:** `referencing` joins `ArraySpec` (per-axis + value);
  `world_frame` (ADR-0030) is expressed as the `affine_nd` + `space` instance.
- **Gate (future):** descriptor `unit` agrees with axis `unit`; `affine_*` non-degenerate; `lookup` length
  matches axis extent; `epoch`/`space` drawn from the registered frame vocabulary.

## Status note
Proposed; supersedes the would-be separate units (#219) and time (#220) ADRs by unifying them. The one
empirical/registry input is the **frame & unit vocabularies** (the named `space`/`epoch`/baseline set +
the UCUM subset we validate), pinned with the 0026–0032 design set as it promotes to **Accepted** (at
which point the descriptor must be cited in FEATURE-MATRIX per the matrix gate).
