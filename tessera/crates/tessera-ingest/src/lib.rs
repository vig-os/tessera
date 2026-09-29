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
/// The ingest conformance corpus (ADR-0056 §5) — fixtures whose golden hashes are pinned in
/// `corpus/ingest-corpus.json`. Feature-gated, with a declared per-configuration fixture count so a
/// vacuous run cannot report green (ADR-0057 §5).
#[cfg(feature = "parquet")]
pub mod corpus;
/// CSV/TSV → `table`, inference-free under an operator-declared schema (ADR-0056 §8). Feature `csv`.
#[cfg(feature = "csv")]
pub mod csv_table;
/// The sealed decoder identity — ADR-0056 §6a's build-honest triple in the provenance recipe bag.
/// The decode-path classification behind the per-lane decoder digest (ADR-0056 §6a, #477): which crates
/// each ingest lane's digest covers, and — with a reason each — which candidates it deliberately does not.
///
/// Shared verbatim with `build.rs` (which `include!`s it) and with the gate test, so the digest and the
/// gate cannot disagree about what the decode path is. The file's own header comment carries the
/// measurements behind "derive the candidates, declare the membership".
pub mod decode_path;
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
