//! # tessera-io — bytes on disk for Tessera
//!
//! `tessera-core` owns the format/spine (manifest, identity, hashing) with no I/O. This crate
//! owns the **`.tsra` container** (a STORED zip64 archive, ADR-0022) and the **read path** —
//! built first, before the write engine, so the format is never reader-hostile. The streaming
//! write engine (P3 / S17) lands here later.

pub mod conformance;
pub mod container;

pub use container::{pack, pack_dir, unpack, BlockPayload, Reader, MIMETYPE};
