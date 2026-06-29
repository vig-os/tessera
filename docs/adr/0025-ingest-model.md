# ADR-0025 — Ingest model: normalise at the door, lossless, provenance-rooted

Status: **Accepted** (2026-06-26) · Implements ROADMAP **P5** (`#207` DICOM, then `#208` vendor raw).
Relates to ADR-0020 (identity/provenance), ADR-0023 (array payload).

## Context
Tessera is substrate-agnostic, but acquisition data arrives in vendor-proprietary formats (DICOM,
GE-HDF5, Siemens binary, raw `.dat`/`.BLF`, NIfTI). The engine must never carry vendor quirks
downstream. A separate crate, **`tessera-ingest`**, decodes each format **at the door** into a normal
Tessera product.

## Decision
1. **Lossless, native dtype.** A decoder preserves the source's native sample dtype (e.g. DICOM CT →
   `int16`); it does **not** bake derived transforms into the samples. For DICOM the rescale
   slope/intercept is read with `ModalityLutOption::None` (raw stored values) and carried as block
   metadata (`rescale_slope`/`rescale_intercept` + UCUM `unit` via the modality→unit map, CT→HU,
   PT→Bq·mL⁻¹) so a reader recovers physical units exactly, reversibly.
2. **Shape via the Tessera spine.** Decoders emit a neutral intermediate (`DicomImage` = shape +
   native voxels + modality + rescale) and a `to_recon_product` builder that goes through the *same*
   `ProductBuilder` + `array_block` as any other producer — so an ingested product is indistinguishable
   from a natively-authored one (same identity/seal rules).
3. **Provenance-rooted.** Every ingested product records a `Source { role: "ingested_from",
   reference: <SOP Instance UID / source path> }` edge, anchoring the Merkle DAG to the acquisition.
4. **Pure-Rust decode.** `dicom-rs` with the C++ codecs (`charls`/`gdcm`) and the network stack
   (`ul`) disabled: uncompressed + RLE transfer syntaxes are pure-Rust, keeping the hermetic build
   clean. JPEG-compressed transfer syntaxes are a later, feature-gated addition.

## Consequences
- New workspace crate `tessera-ingest` (`dicom` module). First cut: single-frame DICOM →
  `recon` product, hermetically tested by **synthesizing** a DICOM in-process (no external fixtures /
  PHI). Real DUPLET DICOM is for manual benching only.
- **Pending under P5:** multi-slice **series stacking** (sort by ImagePositionPatient / InstanceNumber
  into a 3-D volume), **PS3.15 de-identification** verification, a golden DICOM corpus, then GE-HDF5 /
  Siemens / raw / NIfTI (`#208`). These complete the **v0.2** milestone.

## Revision (2026-06-29) — generalised by ADR-0035

The single-product, hardcoded-callsite shape this ADR sketched ("DICOM → recon → write") has been
extended to a **declarative, multi-product** shape by [ADR-0035](0035-declarative-ingest-spec.md):
a TOML spec describes the collection + the derivation DAG; a general engine dispatches each
product to the existing per-backend readers/builders (DICOM / GE-HDF5 / NIfTI / raw). The
per-backend `to_*_product` signatures grew an `extra_sources` slice so the engine can thread
typed `derived_from` (pinned to the parent's `manifest_hash`) + `ingested_via_spec` (pinned to the
spec's `content_hash`) edges. The format on disk is unchanged — the conformance corpus is
untouched.
