//! Parquet → `table` (ADR-0056 §11's P1 row) — the reader half; the whole type map is shared with
//! every other Arrow-shaped source in [`crate::arrow_table`].
//!
//! # There is no byte-level Parquet→Vortex mapping, and we do not want one
//!
//! Ingest is a **logical re-encode** (ADR-0056 §2): `source → RecordBatch → canonicalise →
//! ColumnData → Vortex → seal`. A byte copy of Parquet's column chunks would import the source
//! writer's encoding — its dictionary ordering, its page boundaries, its compression choice, its
//! statistics — into `content_hash`, and thereby defeat the seal. Re-encoding under Tessera's own
//! deterministic codecs *is* the product: it is what buys cross-arch reproducibility, ROI/projection
//! reads and chunk-level integrity, none of which a preserved Parquet byte-stream has.
//!
//! The practical consequence worth stating for a user: the source's compression codec is irrelevant to
//! the output. A snappy Parquet and a zstd Parquet of the same logical table seal to the **same**
//! `content_hash`, and that is a feature, not a coincidence.
//!
//! # What this module deliberately does not do
//!
//! It reads the whole file into memory before sealing. Generic ingest has no streaming path in P1:
//! the multi-block streaming writer exists (`tessera_io::pack_streaming`, used by the GE-HDF5 lane) but
//! wiring a row-group-at-a-time generic ingest onto it is a separate, measurable piece of work rather
//! than something to smuggle in here. Tracked as #458; until then a multi-GB Parquet should go
//! through `--spec` with the vendor path or be split by the producer.

use std::path::Path;

use arrow_array::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use tessera_core::{Error, Result};

use crate::canonical::CanonicalTable;

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ingest: parquet: {e}"))
}

/// Rows per `RecordBatch` handed to the canonicaliser.
///
/// **Not** a determinism input: the canonicaliser folds every batch into one column before anything is
/// encoded, so the batch size cannot reach `content_hash` — which is what
/// `tests/generic_table.rs::the_reader_batch_size_does_not_reach_the_hash` pins, because a reader
/// would otherwise be right to suspect it could. Chosen purely to bound peak RSS during the fold.
const BATCH_ROWS: usize = 8192;

/// Read a Parquet file into `RecordBatch`es.
pub fn read_parquet(path: &Path) -> Result<Vec<RecordBatch>> {
    read_parquet_batched(path, BATCH_ROWS)
}

/// [`read_parquet`] with an explicit batch size — the test seam that proves the batch size is not a
/// determinism input.
pub fn read_parquet_batched(path: &Path, batch_rows: usize) -> Result<Vec<RecordBatch>> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| {
        he(format!(
            "{} is not a readable Parquet file: {e}\n  \
             if it is Arrow IPC / Feather:  tessera ingest table <FILE> --from arrow\n  \
             if it is un-parseable:        tessera ingest blob <FILE>",
            path.display()
        ))
    })?;
    let reader = builder
        .with_batch_size(batch_rows)
        .build()
        .map_err(|e| he(format!("open {} for reading: {e}", path.display())))?;
    let mut out = Vec::new();
    for batch in reader {
        out.push(batch.map_err(|e| he(format!("read {}: {e}", path.display())))?);
    }
    Ok(out)
}

/// A **lazy** batch reader — the streaming counterpart to [`read_parquet_batched`], which collects.
///
/// The collecting version is the memory problem #458 exists to fix: it materialises every row group
/// before anything is sealed, so peak RSS tracks the decoded file rather than one batch. This yields
/// batches one at a time, leaving the caller to decide what to retain.
///
/// Returns a fresh reader on each call, because streaming needs **two** traversals: nullability is a
/// whole-file property (ADR-0029 by-presence), so the shape pass must finish before the encode pass can
/// know what schema to declare. Parquet is seekable by construction — its footer is at the end — so a
/// second pass costs I/O, never correctness.
pub fn parquet_batches(
    path: &Path,
    batch_rows: usize,
) -> Result<impl Iterator<Item = Result<RecordBatch>> + use<>> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| {
        he(format!(
            "{} is not a readable Parquet file: {e}\n  \
             if it is Arrow IPC / Feather:  tessera ingest table <FILE> --from arrow\n  \
             if it is un-parseable:        tessera ingest blob <FILE>",
            path.display()
        ))
    })?;
    let display = path.display().to_string();
    let reader = builder
        .with_batch_size(batch_rows)
        .build()
        .map_err(|e| he(format!("open {} for reading: {e}", path.display())))?;
    Ok(reader.map(move |b| b.map_err(|e| he(format!("read {display}: {e}")))))
}

/// The decoded row count and per-row width Parquet's footer reports, for the streaming threshold.
///
/// Metadata only — no page is decoded. Reading the footer to decide **whether to stream** is sound even
/// though the same footer's `null_count` is deliberately NOT trusted to decide nullability (#502): a
/// wrong estimate here picks a slower code path, and the batch-equals-stream test pins that both paths
/// seal the same bytes, so routing cannot change a seal. Identity gets a stricter standard than routing.
pub fn parquet_size_estimate(path: &Path) -> Result<u64> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| {
        he(format!(
            "{} is not a readable Parquet file: {e}",
            path.display()
        ))
    })?;
    let md = builder.metadata();
    let rows: i64 = md.file_metadata().num_rows();
    // Uncompressed size across every column chunk — what a decode actually has to hold, unlike the
    // on-disk size, which compression can understate by an order of magnitude.
    let bytes: i64 = md.row_groups().iter().map(|rg| rg.total_byte_size()).sum();
    Ok(u64::try_from(bytes.max(rows)).unwrap_or(0))
}

/// Read + canonicalise a Parquet file into a flat Tessera table.
pub fn read_table(path: &Path, exclude: &[String]) -> Result<CanonicalTable> {
    let batches = read_parquet(path)?;
    crate::arrow_table::canonicalise_batches(&batches, exclude)
}
