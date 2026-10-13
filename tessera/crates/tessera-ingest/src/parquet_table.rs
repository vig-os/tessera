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

/// Explain a failure to open `path` as Parquet, naming the cause an operator can act on.
///
/// Parquet describes its schema and row groups in a **footer**, so a reader must seek to the end of
/// the file before it can read the beginning — which a pipe, fifo or `<(…)` process substitution
/// cannot do. The underlying arrow error for that case says only that the file is not Parquet, which
/// sends the operator to look at their data when the problem is their shell. The CSV lane has no
/// footer and does not care, so this asymmetry is real and worth spelling out rather than inferring.
fn not_parquet(path: &Path, e: impl std::fmt::Display) -> tessera_core::Error {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return he(format!(
            "{} is not a regular file, and Parquet cannot be read from a stream: its schema and row \
             groups live in a FOOTER, so the reader must seek to the end before it can read the \
             start. Materialise it first (`zcat data.parquet.gz > data.parquet`) and ingest that.\n  \
             underlying error: {e}",
            path.display()
        ));
    }
    he(format!(
        "{} is not a readable Parquet file: {e}\n  \
         if it is Arrow IPC / Feather:  tessera ingest table <FILE> --from arrow\n  \
         if it is un-parseable:        tessera ingest blob <FILE>",
        path.display()
    ))
}

/// [`read_parquet`] with an explicit batch size — the test seam that proves the batch size is not a
/// determinism input.
pub fn read_parquet_batched(path: &Path, batch_rows: usize) -> Result<Vec<RecordBatch>> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| not_parquet(path, e))?;
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
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| not_parquet(path, e))?;
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
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| not_parquet(path, e))?;
    let md = builder.metadata();
    let rows: i64 = md.file_metadata().num_rows();
    // Uncompressed size across every column chunk — what a decode actually has to hold, unlike the
    // on-disk size, which compression can understate by an order of magnitude.
    let bytes: i64 = md.row_groups().iter().map(|rg| rg.total_byte_size()).sum();
    Ok(u64::try_from(bytes.max(rows)).unwrap_or(0))
}

/// Lazily canonicalised Parquet chunks — what the streaming driver consumes.
///
/// The canonicalisation lives here, in the lane, so the driver never has to know a lane's decode shape
/// (ADR-0056 §12a keeps arrow out of the CSV lane entirely, so a RecordBatch-shaped contract would not
/// have fit all three). Uses the same `canonicalise_batch` the batch fold uses — reused, not repeated.
pub fn parquet_chunks(
    path: &Path,
    batch_rows: usize,
    exclude: &[String],
) -> Result<impl Iterator<Item = Result<CanonicalTable>> + use<>> {
    let exclude = exclude.to_vec();
    Ok(parquet_batches(path, batch_rows)?
        .map(move |b| b.and_then(|b| crate::arrow_table::canonicalise_batch(&b, &exclude))))
}

/// Read + canonicalise a Parquet file into a flat Tessera table.
pub fn read_table(path: &Path, exclude: &[String]) -> Result<CanonicalTable> {
    let batches = read_parquet(path)?;
    crate::arrow_table::canonicalise_batches(&batches, exclude)
}
