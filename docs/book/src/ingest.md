# Ingesting vendor data

Ingest is the Layer-0 → Layer-1 boundary: normalise a vendor-proprietary file into an open,
content-addressed `.tsra` **at the door**, so you're never hostage to a vendor format or codec. It lives
in the `tessera-ingest` companion crate (the core format stays substrate-agnostic).

Each source seals a verifying product with an `ingested_from` provenance edge back to the original bytes
(hashed, so the edge is tamper-evident). The `ingest` sub-command dispatches by format:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/ingest_spec.trycmd}}

Supported readers (all lossless, native dtype):

- **DICOM** — series → a 3-D `recon` array block; tags → manifest metadata + units + modality; PS3.15
  de-identification (`--deidentify`, or `--crypto-shred` to carry the original identity as an encrypted
  `aux/` envelope). JPEG-Baseline decodes pure-Rust. The golden DICOM ingest is content-hash-pinned.
- **GE-HDF5 listmode** — compound events → columnar table (a generic HDF5-compound decoder); **bounded
  memory** even above RAM (row-slab streaming → fixed-size blocks, byte-identical to a whole-file read).
- **NIfTI-1** — `.nii` → `recon`, with the sform RAS→LPS `world_frame` and `scl_*` rescale.
- **raw** — headerless binary with a caller-supplied shape + dtype (element-count guarded).
- **blob** — any file stored **verbatim** for bit-faithful preservation (see [Anatomy](./format-anatomy.md));
  `tessera extract` returns the exact bytes back, digest-verified.

A **declarative spec** (`ingest --spec <toml>`) drives a multi-dataset ingest (raw + derived products +
a study collection) through one format-tagged engine (ADR-0035), rather than bespoke code per dataset.

*Evidence:* the reader round-trips are covered by `tessera_ingest::{dicom,ge_hdf5,nifti,raw,blob}::tests`
(byte-identical / golden-pinned), run by `cargo test` on every build. Siemens proprietary binary needs
vendor samples to reverse-engineer and is out-of-tree (the same class as real-PHI data).
