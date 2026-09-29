# Product-schema reference

Every `.tsra` declares a **product** — `recon`, `listmode`, `blob`, … — and that product's schema is its
contract: which metadata fields it must carry, which it should, and which blocks play which role. The
schema is **embedded in the sealed file** (ADR-0029), so a product carries its own contract and a reader
never has to guess.

Three tiers, and the difference matters when you are writing your own data:

| tier | what happens if it is absent |
| --- | --- |
| **required** | a hard **block** — `tessera schema <file>` fails and the product is invalid |
| recommended | a non-fatal **warn** on stderr at ingest — the FAIR-completeness nudge |
| optional | nothing |

To check a file you already have, against its own embedded contract:

```console
tessera schema study.tsra          # validate: required fields + block roles
tessera schema study.tsra --json   # dump the file's embedded schema
```

The tables below are the **built-in registry** — what `tessera` knows without being told. They are
generated from `SchemaRegistry::builtin()`, not written beside it, and the `derived-docs` gate fails a
commit where the two disagree. A schema cannot gain a required field without this page gaining it in the
same commit. Regenerate with:

```console
guardrails-derived-docs --fix docs/book/src/schemas.md
```

## The built-in products

<!-- guardrails:derived cmd="cd tessera && cargo run -q -p tessera-core --example schema_reference" -->
<!-- Generated from the built-in SchemaRegistry — do not edit by hand. -->

| product | version | requires a recipe | description |
| --- | --- | --- | --- |
| [`blob`](#blob) | 1.0 | no | Opaque preserved file — bytes stored bit-faithfully, not engine-parsed (the "junk" tier). |
| [`calibration`](#calibration) | 1.0 | no | Scanner calibration data. |
| [`deformation_field`](#deformation-field) | 1.0 | no | A non-linear (deformable) spatial transform — a per-voxel displacement field (ADR-0030 §5). |
| [`device_data`](#device-data) | 1.0 | no | Raw device data from an existing system (GE/Siemens/…). |
| [`diffusion_mri`](#diffusion-mri) | 1.0 | no | A diffusion MRI acquisition: per-direction volumes + b-values/vectors. |
| [`dynamic_pet`](#dynamic-pet) | 1.0 | no | A dynamic (4-D) PET acquisition: one time-series volume + a frame-timing table. |
| [`listmode`](#listmode) | 1.0 | no | Raw per-event list-mode acquisition data. |
| [`multicontrast_mri`](#multicontrast-mri) | 1.0 | no | A multi-contrast MRI study: N separately-acquired contrast volumes (model B). |
| [`recon`](#recon) | 1.0 | no | A reconstructed image volume (CT/PET/μ-map). |
| [`roi`](#roi) | 1.0 | no | Regions of interest over an image product (representation chosen by the ROI's nature). |
| [`sim`](#sim) | 1.0 | no | Monte-Carlo simulation output. |
| [`sinogram`](#sinogram) | 1.0 | no | Projection-space (sinogram) data. |
| [`spectrum`](#spectrum) | 1.0 | no | An energy / positronium-lifetime / TOF histogram. |
| [`transform`](#transform) | 1.0 | no | A spatial transform / registration result. |

### blob

Opaque preserved file — bytes stored bit-faithfully, not engine-parsed (the "junk" tier).

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `study` | recommended | string | — | Public | Study / exam this preserved file belongs to (FAIR grouping) |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `data` | blob | 1 | The preserved source file, stored verbatim as opaque bytes (blake3-verified) |

### calibration

Scanner calibration data.

_No schema metadata fields._

| block role | kind | min | description |
| --- | --- | --- | --- |
| `calibration` | array or table | 1 | Calibration coefficients / lookup (normalization, attenuation, dead-time) |

### deformation_field

A non-linear (deformable) spatial transform — a per-voxel displacement field (ADR-0030 §5).

_No schema metadata fields._

| block role | kind | min | description |
| --- | --- | --- | --- |
| `field` | array | 1 | A dense displacement/deformation vector field, e.g. `[3, z, y, x]` (one array block) |

### device_data

Raw device data from an existing system (GE/Siemens/…).

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `vendor` | optional | string | — | Public | Device vendor |
| `model` | optional | string | — | Public | Device model |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `raw` | array or table | 1 | Raw vendor/device payload (normalized at ingest, preserved verbatim) |

### diffusion_mri

A diffusion MRI acquisition: per-direction volumes + b-values/vectors.

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `modality` | **required** | coded | — | Coded | Imaging modality |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `volume` | array | 1 | 4-D diffusion volume (dir,z,y,x), native dtype |
| `gradients` | table | 1 | Per-direction b-value (s/mm²) + unit b-vector (bval/bvec) |

### dynamic_pet

A dynamic (4-D) PET acquisition: one time-series volume + a frame-timing table.

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `modality` | **required** | coded | — | Coded | Imaging modality |
| `decay_correction_reference` | optional | string | — | Public | Named instant PET activity is decay-corrected to (e.g. injection / scan-start / acquisition-start) |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `volume` | array | 1 | 4-D dynamic volume (t,z,y,x), native dtype — one N-D array, the t_c lever sets per-frame↔TAC locality |
| `frame_timing` | table | 1 | Per-frame start + duration (s, monotonic) and decay-correction reference |

### listmode

Raw per-event list-mode acquisition data.

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `coincidence_mode` | **required** | string | — | Public | Acquisition mode (singles / prompt-coincidence / extended-coincidence) |
| `patient_id` | optional | string | — | Identifying | Pseudonymised patient handle for this acquisition (direct PHI — supply a site pseudonym, never the raw MRN) |
| `exam` | optional | string | — | Identifying | Exam / study number linking the acquisition to its study |
| `study_date` | optional | string | — | Sensitive | Acquisition date (YYYYMMDD or RFC-3339) — scan context, access-controlled |
| `acquisition_start` | optional | string | — | Sensitive | Acquisition start instant |
| `acquisition_stop` | optional | string | — | Sensitive | Acquisition stop instant |
| `acquisition_duration` | optional | float64 | s | Public | Acquisition duration |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `events` | table | 1 | Per-event columnar table (timestamps, energies, positions) |

### multicontrast_mri

A multi-contrast MRI study: N separately-acquired contrast volumes (model B).

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `modality` | **required** | coded | — | Coded | Imaging modality |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `volume` | array | 1 | One image volume per contrast (≥1 — heterogeneous matrix/voxel, e.g. T1/T2/FLAIR) |

### recon

A reconstructed image volume (CT/PET/μ-map).

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `modality` | **required** | coded | — | Coded | Imaging modality |
| `rescale_slope` | optional | float64 | — | Public | Native→physical slope |
| `rescale_intercept` | optional | float64 | — | Public | Native→physical intercept |
| `patient_pseudonym` | optional | string | — | Identifying | Pseudonymised patient handle (DICOM PS3.15 Patient Name / Patient ID family — direct PHI; supply a site-issued pseudonym, never the raw MRN) |
| `acquisition_uid` | optional | string | — | Identifying | Acquisition UID (DICOM PS3.15 UID family — Study/Series/SOPInstance UID; linking PHI under the confidentiality profile) |
| `study_instance_uid` | recommended | string | — | Identifying | DICOM StudyInstanceUID (0020,000D) — PS3.15 UID family, links back to the patient/study |
| `series_instance_uid` | recommended | string | — | Identifying | DICOM SeriesInstanceUID (0020,000E) — PS3.15 UID family, links back to the series |
| `study_date` | recommended | string | — | Sensitive | DICOM StudyDate (0008,0020), YYYYMMDD — scan context, access-controlled |
| `manufacturer` | recommended | string | — | Public | DICOM Manufacturer (0008,0070) — device vendor |
| `model_name` | recommended | string | — | Public | DICOM ManufacturerModelName (0008,1090) — device model |
| `kvp` | recommended | float64 | kV | Public | DICOM KVP (0018,0060), CT peak kilovoltage |
| `slice_thickness` | recommended | float64 | mm | Public | DICOM SliceThickness (0018,0050), nominal slice thickness |
| `pixel_spacing` | recommended | json | mm | Public | DICOM PixelSpacing (0028,0030), [row_spacing, col_spacing] in mm |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `volume` | array | 1 | Reconstructed image volume (z,y,x), native dtype |

### roi

Regions of interest over an image product (representation chosen by the ROI's nature).

_No schema metadata fields._

| block role | kind | min | description |
| --- | --- | --- | --- |
| `roi` | array or table | 1 | Region(s) of interest — a raster label array, or a parametric / contour / stats table |

### sim

Monte-Carlo simulation output.

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `simulator` | optional | string | — | Public | Simulation toolkit (e.g. GATE/Geant4) |
| `seed` | optional | int64 | — | Public | RNG seed for reproducibility |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `output` | array or table | 1 | Simulation output (hits table or scored volume) |

### sinogram

Projection-space (sinogram) data.

_No schema metadata fields._

| block role | kind | min | description |
| --- | --- | --- | --- |
| `sinogram` | array | 1 | Projection / sinogram array (angle, radial, plane) |

### spectrum

An energy / positronium-lifetime / TOF histogram.

| field | tier | dtype | unit | sensitivity | description |
| --- | --- | --- | --- | --- | --- |
| `domain` | optional | string | — | Public | Histogram domain (energy / lifetime / time-of-flight) |

| block role | kind | min | description |
| --- | --- | --- | --- |
| `spectrum` | array | 1 | Histogram of an energy/lifetime/TOF quantity — a dense 1-D array by nature (ADR-0029 §6) |

### transform

A spatial transform / registration result.

_No schema metadata fields._

| block role | kind | min | description |
| --- | --- | --- | --- |
| `transform` | array or table | 1 | Spatial transform: affine (table) or deformation field (array) |
<!-- guardrails:derived:end -->

## Open-world products

A product name the registry does not know is **allowed** — the field stays absent from the seal and
nothing validates it. That is deliberate (ADR-0029's permissive escape hatch): a domain Tessera has never
heard of should not have to patch the registry to store its data. You lose the schema guarantees, not the
ability to seal.
