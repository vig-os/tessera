//! # tessera-io — bytes on disk for Tessera
//!
//! `tessera-core` owns the format/spine (manifest, identity, hashing) with no I/O. This crate
//! owns the **`.tsra` container** (a STORED zip64 archive, ADR-0022) and the **read path** —
//! built first, before the write engine, so the format is never reader-hostile. The streaming
//! write engine (P3 / S17) lands here later.

pub mod accumulate;
pub mod array;
pub mod chunk_index;
#[cfg(feature = "cloud")]
pub(crate) mod cloud;
pub mod collection;
pub mod conformance;
pub mod container;
pub mod oci;
pub mod range;
pub mod sign;
pub mod stream;
pub mod table;
pub mod worm;
pub mod write;

pub use accumulate::TableStreamWriter;
pub use array::{decode, decode_subset, encode, ArrayData};
pub use collection::{
    prefix_layout, retention_mode, to_oci_index, to_rocrate, OCI_INDEX_MEDIA_TYPE,
};
pub use container::{pack, pack_dir, unpack, BlockPayload, Reader, MIMETYPE};
pub use range::CountingReader;
pub use sign::{sign_tsra, verify_tsra};
pub use stream::{array_job, table_job, EncodeJob, StreamWriter};
pub use table::{ColumnData, TableData};
pub use write::WriteSession;
