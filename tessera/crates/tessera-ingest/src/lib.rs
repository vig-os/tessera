//! # tessera-ingest — normalise vendor acquisition formats into Tessera products
//!
//! Per the architecture (ROADMAP P5): proprietary formats are decoded **at the door** into the
//! substrate-agnostic Tessera model — never the engine's concern downstream. DICOM lands first
//! (#207); GE-HDF5 / Siemens / raw `.dat`/`.BLF` / NIfTI follow (#208). Each decoder is lossless:
//! it preserves the native sample dtype and carries the provenance + units as metadata rather than
//! rewriting pixels.

pub mod blob;
pub mod dicom;
pub mod engine;
pub mod ge_hdf5;
pub mod identity;
pub mod nifti;
pub mod provenance;
pub mod raw;
pub mod spec;
