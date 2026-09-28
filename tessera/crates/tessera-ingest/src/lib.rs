//! # tessera-ingest — normalise vendor acquisition formats into Tessera products
//!
//! Per the architecture (ROADMAP P5): proprietary formats are decoded **at the door** into the
//! substrate-agnostic Tessera model — never the engine's concern downstream. DICOM lands first
//! (#207); GE-HDF5 / Siemens / raw `.dat`/`.BLF` / NIfTI follow (#208). Each decoder is lossless:
//! it preserves the native sample dtype and carries the provenance + units as metadata rather than
//! rewriting pixels.

/// The ADR-0056 §2 Arrow→Tessera type map — the table lane's canonicalisation, plus the Arrow IPC
/// reader. Feature `arrow` (ADR-0057 §3 keeps arrow out of `tessera-core` entirely).
#[cfg(feature = "arrow")]
pub mod arrow_table;
pub mod backends;
pub mod blob;
/// The generic-ingest canonicalisation boundary (ADR-0056 §2, ADR-0057 §1) — shared by every
/// non-vendor backend. Not feature-gated: the boundary is ours, only the decoders that feed it are.
pub mod canonical;
/// Operator-declared per-column semantics (`--column-meta`, ADR-0056 §7).
pub mod column_meta;
/// CSV/TSV → `table`, inference-free under an operator-declared schema (ADR-0056 §8). Feature `csv`.
#[cfg(feature = "csv")]
pub mod csv_table;
/// The sealed decoder identity — ADR-0056 §6a's build-honest triple in the provenance recipe bag.
pub mod decoder;
pub mod dicom;
pub mod engine;
pub mod ge_hdf5;
pub mod identity;
pub mod nifti;
/// Parquet + Arrow IPC → `table` (ADR-0056 §11). Feature `parquet`; the type map is
/// [`crate::arrow_table`]'s.
#[cfg(feature = "parquet")]
pub mod parquet_table;
pub mod provenance;
pub mod raw;
pub mod spec;
