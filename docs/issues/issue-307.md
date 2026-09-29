---
type: issue
state: closed
created: 2026-07-02T15:01:50Z
updated: 2026-09-28T17:54:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/307
comments: 3
labels: feature, discussion, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:50.991Z
---

# [Issue 307]: [feat(core): Vortex table Column carries no unit/description/short_name — fd5 I1/I2 gap on the data tier](https://github.com/vig-os/tessera/issues/307)

## Context — first-user DUPLET FAIR shakedown

Reviewing whether tessera honors the fd5 per-field annotation triad (**short name · description ·
unit**) on the actual **Vortex table** and **Zarr array** data — not just scalar metadata.

## Findings

**`FieldSpec` (schema.rs) has the full triad** — `id` (rename-safe short name), `description`,
`dtype`, **UCUM `unit`** ("HU", "Bq/mL", "ps", "keV"), `vocabulary`. Its doc even promises "a reader
(or an AI) always has the field's meaning, unit, and dtype without external context (FAIR I1/I2)."
**But that applies only to product-level *metadata* fields** (modality, kvp, …).

**Zarr arrays — partial.** The array block carries a `unit` (DICOM path: `unit_for(modality)` →
`with_unit("HU"/"Bq/mL")`), a block `name`, and a schema-level block `description`. Reasonable for the
DICOM backend; a `raw`/`nifti`/`hdf` array gets no unit unless supplied.

**Vortex tables — NO.** The column type is:
```rust
pub struct Column { pub name: String, pub dtype: String, pub codec: Option<String> }
```
No `unit`, no `description`, no long/short name. The `FieldSpec` triad is **never wired to table
columns** — the bulk of the scientific data. `ge-hdf5` copies the terse HDF5 compound member names
verbatim, and HDF5 carries no units/descriptions, so columns land bare. Concretely, from the DP01
Singles `.h5`:

| dataset | columns (bare names, no unit/description) |
|---|---|
| `raw_data/time_markers` | `tm, idx, length, n_singles, n_coins2p, n_coins3p, n_event2p, n_event3p` |
| `raw_data/coin_counters` | `tm, idx, cc` |
| `proc_data/events_*` | 6–8 terse fields (energies/positions/time) |

A downstream user/AI sees `tm : u8` with no hint it's a time-marker timestamp, its unit (ps? clock
ticks?), or meaning. That's an **I1/I2 FAIR gap for the table tier.**

## Ask

1. Extend `Column` with optional `unit` / `description` / `short_name` (mirror `FieldSpec`), covered by
   `manifest_hash` like every other field. `#[serde(default)]` keeps on-disk back-compat.
2. Surface them in `ls` / `stats` / `schema` / `export` (FAIR discovery record).
3. Populate at ingest from a **column-definitions source** — since the vendor HDF5 lacks them, a
   per-vendor/per-dataset dictionary is needed (GE listmode `tm/idx/e0/e1/n_singles/cc/…` → unit +
   description). Authoritative GEDDF definitions must come from the domain owner / GE docs (the public
   MorePET repos checked did not carry a field dictionary).

Found during: DUPLET first-user FAIR ingest (DP01). Relates to #300 (array rescale unit metadata),
#305 (listmode table modeling).

---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 03:09 PM_

## GEDDF listmode column dictionary — sourced from the domain owner

Built the per-column definitions the vendor HDF5 omits, from the authoritative sources:

- **GE `petCoincLinkEvents.h`** © 2012 General Electric — raw Galileo coincidence-link bitfields
  (crystal axial/trans-axial IDs, Anger-math energies, signed TOF Δt, time markers, coinc counters).
- **`MorePET/geddf-code-tim`** — `listParse.cpp` (raw→parsed), `getEnergyAxis_DMI6.m`
  (`keV = ((bin−0.5)/EPeak)×511`), `parseRawListFile.m`. Geometry: **54 axial × 544 trans-axial** (DMI6).

Full dictionary (shaped like `FieldSpec`: short_name · description · unit · vocabulary · dtype) →
**`tessera/docs/dictionaries/ge-discovery-mi-listmode.toml`**. Highlights:

| dataset | column | meaning | unit |
|---|---|---|---|
| events_2p/3p | `en` | calibrated per-photon energies [2]/[3] | **keV** |
| events_2p/3p | `vtx` | reconstructed annihilation vertex (x,y,z) | **mm** ⚠ |
| events_3p | `lt` / `lt_corr` | **positronium lifetime** (raw / corrected) | **ps** ⚠ |
| events/coin | `dt` | signed TOF Δt (electronics bins → ps via cal) | {tof_bin} |
| coin/singles | `en` | Anger-math energy (raw bin → keV) | {energy_bin} |
| all | `ax` / `tx` | crystal axial (0–53) / trans-axial (0–543) index | 1 |
| singles | `ms` / `td` | system-clock timestamp / fine time detail | 1 ⚠ |
| time_markers | `tm`,`idx`,`length`,`n_singles`,`n_coins2p/3p`,`n_event2p/3p` | block clock + offset + interval counts | 1 |
| coin_counters | `tm`,`idx`,`cc` | time marker + offset + HW coinc count (20-bit) | 1 |

**⚠ 9 fields flagged `_tentative` for domain-owner confirmation:**
- `vtx` unit = mm (assumed scanner FOV coords — confirm frame/origin).
- `lt`/`lt_corr` unit = ps (could be ns).
- `ms` (all) — exact clock unit/rate (ticks vs ms) for wall-clock conversion.
- `singles.td` — meaning (fine-time vs other) uncertain.
- `time_markers.length` — interval semantics.

Once confirmed, this TOML is the column-definitions source the ingest reads to populate `Column`
`unit`/`description`/`short_name` (the ask in this issue).


---

# [Comment #2]() by [gerchowl]()

_Posted on July 2, 2026 at 03:16 PM_

## Authoritative source found + units confirmed by range analysis

`geddf-code-tim` is MATLAB/C++ only (no Python). The **canonical** source is
**`MorePET/ge-discovery-data-framework`** — the parser that *wrote* this HDF5:
- `docs/DescriptionCSV.md` — the field dictionary (`ID, E/eV, t/ps, N, V_{x,y,z}/mm, L/ps`)
- `lib/DiscoveryMIGen2/include/ListModeEvent.h` — field defs + unit comments (`timeMarker "in ms"`,
  `coincCount = # coincidences in prior 1ms window`)
- `docs/DOC1170822 - CoinLink White Paper.pdf` (GE's format spec), `lib/GEHDF5/` (the writer)

Cross-checked every unit against a 500k-row range analysis of DP01. Confirmed: **`vtx`=mm** (x,y ±369.8
= transaxial FOV, z ±147.9 = axial FOV), **`ms`/`tm`=ms** (+1/row, ~420 s acq), **`td`= time-to-next-hit
in ps** (non-rolling Δt), **`length`= ms-span of the marker** (~1; >1 across data-loss gaps).

**Two doc-vs-data conflicts — resolved to the data:**
- **`lt` (lifetime): data ±17 (matches the ±15 ns coincidence window) ⇒ `ns`**, though DescriptionCSV
  says "ps". (`lt_corr` is mostly NaN — sparse.)
- **`en` (event energy): data peaks at 511 (annihilation photopeak), 425–650 ⇒ `keV`**, though
  DescriptionCSV says "eV". (3p `en[2]` = prompt γ, 600–1220 keV.)

Dictionary at `tessera/docs/dictionaries/ge-discovery-mi-listmode.toml` updated accordingly (conflicts
tagged `_conflict`). Remaining `_tentative`: `singles.en`/`coin.en` (integer readout — keV vs raw bin).

Other org sources not yet mined: `geddf-analysis`, `student-projects`, `2023-04-11_Thesis_BryanBenz`.


---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 49224a5 'feat(core): table Column carries unit/description/short_name/scale (#307) (#312)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

