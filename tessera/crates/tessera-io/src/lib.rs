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
pub mod cloud;
pub mod collection;
pub mod config;
pub mod conformance;
pub mod container;
pub mod multiblock;
pub mod oci;
pub mod range;
#[cfg(feature = "cloud")]
pub mod registry;
pub mod repo;
pub mod sign;
pub mod stream;
pub mod table;
pub mod worm;
pub mod write;

pub use accumulate::{TableMultiBlockSink, TableStreamWriter};
pub use array::{decode, decode_subset, encode, ArrayData};
#[cfg(feature = "cloud")]
pub use cloud::{open_url, ObjectStoreReader, TAIL_PREFETCH};
pub use collection::{
    prefix_layout, retention_mode, to_oci_index, to_rocrate, OCI_INDEX_MEDIA_TYPE,
};
pub use config::{parse_byte_size, WriteConfig, DEFAULT_RAM_BUDGET};
pub use container::{pack, pack_dir, pack_streaming, unpack, BlockPayload, Reader, MIMETYPE};
pub use multiblock::{ColumnBlockIter, LogicalTableView};
pub use range::CountingReader;
#[cfg(feature = "cloud")]
pub use registry::{pull as registry_pull, push as registry_push};
pub use repo::{GcReport, LogEntry, Repository};
pub use sign::{sign_tsra, verify_tsra};
pub use stream::{array_job, table_job, table_job_from_fragments, EncodeJob, StreamWriter};
pub use table::{block_count, block_name, partition_blocks, ColumnData, TableData, BLOCK_ROWS};
pub use write::WriteSession;
