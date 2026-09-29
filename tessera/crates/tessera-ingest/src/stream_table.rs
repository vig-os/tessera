//! Two-pass streaming for the generic table lanes (#458) — bounded memory instead of whole-file.
//!
//! # Why two passes, and not one
//!
//! Nullability is **by presence** (ADR-0029), decided over the whole input: a column seals as
//! `ColumnData::Nullable` iff *some* row in the file is null. `arrow_table`'s fold says so outright —
//! "a column whose only null is in the last row group still seals as one nullable column" — and
//! `csv_table` agrees, which is why CSV's declared `nullable: true` is a *permission* rather than the
//! seal.
//!
//! That makes nullability a property of the file, while `Nullable { values, validity }` is a
//! **representation** rather than a flag, and [`tessera_io::TableMultiBlockSink::new`] takes the column
//! schema up front because every block must carry the same one. So the first block cannot be encoded
//! until the last row has been read. A single-pass stream would have to either seal a different schema
//! per block, or decide nullability from a prefix of the file — and the second silently moves
//! `content_hash` away from what a batch ingest of the same bytes produces, which is the one thing
//! #458 may not do.
//!
//! Hence: **pass 1 reads the shape, pass 2 encodes.** Pass 1 keeps a set of leaf names and a column
//! schema; the data of each batch is dropped as soon as it has been inspected, so memory is bounded by
//! one batch either way.
//!
//! The Parquet footer's per-chunk `null_count` would make pass 1 nearly free, and is deliberately not
//! used: statistics are optional and are the *producer's* claim, so a wrong or missing one would move a
//! sealed `content_hash` silently. Tracked as #502, gated on proof-of-agreement rather than trust.
//! (Choosing *whether* to stream may read the footer — see `engine`'s threshold probe — because a bad
//! estimate only picks a slower path and never changes a seal. Identity and routing get different
//! levels of trust from the same bytes.)
//!
//! # Why this agrees with the batch path by construction
//!
//! Both passes call the **same** [`crate::arrow_table::canonicalise_batch`] the batch fold calls, so
//! nothing about the mapping, the rejection rules or `--exclude` is reimplemented here. Two properties
//! make per-batch canonicalisation equal to canonicalise-then-concatenate:
//!
//! - [`crate::canonical::canonicalise`] is **element-wise** (NaN normalisation and masked-slot zeroing),
//!   so applying it per batch and concatenating gives the same values as concatenating and applying it
//!   once.
//! - `concat_columns` promotes to `Nullable` when *either* side is, so "some batch produced `Nullable`
//!   for this leaf" is exactly the fold's verdict.
//!
//! The transform receipt is deduplicated by `(name, column)` in a `BTreeMap`
//! ([`crate::canonical::TableBuilder::record`]), so merging every batch's records into one builder
//! yields the batch path's list — same entries, same order. Recording per batch *without* merging would
//! instead emit one entry per batch and move `manifest_hash`, which is the subtle failure this module's
//! tests pin.

use std::collections::{BTreeMap, BTreeSet};

use tessera_core::block::table::Column;
use tessera_core::{Error, IngestTransform, Result};
use tessera_io::table::ColumnData;

use crate::canonical::CanonicalTable;

/// The file-wide shape pass 1 establishes: the column schema every block will carry, and the transform
/// records observed while reading it.
#[derive(Debug, Default)]
pub struct FileShape {
    /// Columns in source order, with **file-wide** nullability applied.
    pub columns: Vec<Column>,
    /// Leaf names that sealed as `Nullable` in at least one batch.
    pub nullable: BTreeSet<String>,
    /// Rows seen, so the caller can decide the block partition and report progress.
    pub rows: u64,
    /// Transform records observed in pass 1, deduplicated exactly as the batch path does.
    pub transforms: Vec<IngestTransform>,
}

/// Accumulates [`FileShape`] over a sequence of already-canonicalised batches.
///
/// Separate from the readers so it can be driven by any lane — and tested without a file.
#[derive(Debug, Default)]
pub struct ShapeScan {
    columns: Vec<Column>,
    nullable: BTreeSet<String>,
    rows: u64,
    transforms: BTreeMap<(String, String), IngestTransform>,
}

impl ShapeScan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inspect one canonicalised batch and drop its data.
    ///
    /// Leaf identity is checked across batches for the same reason the batch fold checks it: a source
    /// whose columns change shape between chunks cannot be one table, and discovering that at block 400
    /// rather than at the second batch would waste the whole read.
    pub fn push(&mut self, table: &CanonicalTable) -> Result<()> {
        if self.columns.is_empty() {
            self.columns = table.columns.iter().map(|(c, _)| c.clone()).collect();
        } else if self.columns.len() != table.columns.len() {
            return Err(shape_error(format!(
                "a chunk expanded to {} columns where an earlier one gave {}",
                table.columns.len(),
                self.columns.len()
            )));
        } else {
            for (have, (got, _)) in self.columns.iter().zip(&table.columns) {
                if have.name != got.name {
                    return Err(shape_error(format!(
                        "a column changed name between chunks ('{}' → '{}')",
                        have.name, got.name
                    )));
                }
            }
        }
        for (column, data) in &table.columns {
            if matches!(data, ColumnData::Nullable { .. }) {
                self.nullable.insert(column.name.clone());
            }
        }
        self.rows += table.rows() as u64;
        for t in &table.transforms {
            let col = t
                .params
                .get("column")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            self.transforms
                .entry((t.name.clone(), col))
                .or_insert_with(|| t.clone());
        }
        Ok(())
    }

    /// The file-wide shape: every column that was nullable in **any** batch is nullable in the schema.
    pub fn finish(self) -> FileShape {
        let nullable = self.nullable;
        let columns = self
            .columns
            .into_iter()
            .map(|c| {
                if nullable.contains(&c.name) {
                    c.nullable()
                } else {
                    c
                }
            })
            .collect();
        FileShape {
            columns,
            nullable,
            rows: self.rows,
            transforms: self.transforms.into_values().collect(),
        }
    }
}

/// Coerce one canonicalised batch to the file-wide schema, so every block encodes alike.
///
/// A batch with no nulls in a column the *file* nulls somewhere must still carry a validity mask, or its
/// block would encode a different column type than its neighbours. [`crate::canonical::promote_nullable`]
/// is the same helper `concat_columns` uses for this, reused rather than repeated.
///
/// Returns the per-column data in schema order. Errors if a batch disagrees with the shape pass — which
/// can only happen if the input changed underneath us, and is worth saying so rather than sealing a
/// mixture.
pub fn coerce_to_shape(shape: &FileShape, table: CanonicalTable) -> Result<Vec<ColumnData>> {
    if table.columns.len() != shape.columns.len() {
        return Err(shape_error(format!(
            "a batch has {} columns but the shape pass found {} — did the input change?",
            table.columns.len(),
            shape.columns.len()
        )));
    }
    let mut out = Vec::with_capacity(table.columns.len());
    for (declared, (column, data)) in shape.columns.iter().zip(table.columns) {
        if declared.name != column.name {
            return Err(shape_error(format!(
                "a batch's column '{}' does not match the shape pass's '{}' — did the input change?",
                column.name, declared.name
            )));
        }
        out.push(if shape.nullable.contains(&declared.name) {
            crate::canonical::promote_nullable(data)
        } else {
            data
        });
    }
    Ok(out)
}

fn shape_error(what: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("stream-ingest: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::TableBuilder;

    /// Build a one-column canonical table, optionally with a null, the way a lane's batch would arrive.
    fn batch(name: &str, values: Vec<i32>, null_at: Option<usize>) -> CanonicalTable {
        let mut b = TableBuilder::new();
        let data = match null_at {
            None => ColumnData::I32(values),
            Some(i) => {
                let mut validity = vec![true; values.len()];
                validity[i] = false;
                ColumnData::Nullable {
                    values: Box::new(ColumnData::I32(values)),
                    validity,
                }
            }
        };
        let column = Column::new(name, "i4");
        let column = if matches!(data, ColumnData::Nullable { .. }) {
            column.nullable()
        } else {
            column
        };
        b.push(column, data).unwrap();
        b.finish()
    }

    /// **The load-bearing case.** A column whose only null is in the LAST batch must seal as nullable —
    /// and therefore every earlier block must carry a validity mask too, or the blocks disagree.
    ///
    /// This is what a single-pass stream gets wrong: having seen no null in batch 1, it would seal that
    /// block non-nullable and only discover at batch N that the column was nullable all along. The batch
    /// path decides over the whole fold, so the two would disagree on `content_hash` — the one outcome
    /// #458 forbids.
    #[test]
    fn a_null_only_in_the_last_batch_makes_every_block_nullable() {
        let mut scan = ShapeScan::new();
        scan.push(&batch("x", vec![1, 2], None)).unwrap();
        scan.push(&batch("x", vec![3, 4], None)).unwrap();
        scan.push(&batch("x", vec![5, 6], Some(1))).unwrap();
        let shape = scan.finish();

        assert!(
            shape.nullable.contains("x"),
            "the late null makes the column nullable"
        );
        assert!(
            shape.columns[0].nullable,
            "…and the declared schema says so"
        );
        assert_eq!(shape.rows, 6);

        // Now pass 2: the FIRST batch, which contains no null at all, must still be coerced.
        let coerced = coerce_to_shape(&shape, batch("x", vec![1, 2], None)).unwrap();
        assert!(
            matches!(coerced[0], ColumnData::Nullable { .. }),
            "a null-free batch must still carry a mask, or its block encodes a different column type"
        );
        match &coerced[0] {
            ColumnData::Nullable { validity, .. } => assert_eq!(validity, &[true, true]),
            other => panic!("expected Nullable, got {other:?}"),
        }
    }

    /// With no null anywhere, nothing is promoted — so a file that has never needed a mask does not
    /// gain one, and its blocks stay byte-identical to the batch path's single block.
    #[test]
    fn a_file_with_no_nulls_promotes_nothing() {
        let mut scan = ShapeScan::new();
        scan.push(&batch("x", vec![1, 2], None)).unwrap();
        scan.push(&batch("x", vec![3, 4], None)).unwrap();
        let shape = scan.finish();
        assert!(shape.nullable.is_empty());
        assert!(!shape.columns[0].nullable);
        let coerced = coerce_to_shape(&shape, batch("x", vec![1, 2], None)).unwrap();
        assert!(
            matches!(coerced[0], ColumnData::I32(_)),
            "no mask was added"
        );
    }

    /// The transform receipt is merged, not accumulated per batch.
    ///
    /// `TableBuilder::record` dedupes by `(name, column)`, so a transform that fires in every batch must
    /// appear ONCE — exactly as it does when the batch path canonicalises one folded column. Emitting it
    /// per batch would move `manifest_hash` away from the batch path while leaving `content_hash` intact,
    /// which is precisely the kind of divergence that would not show up in a content-hash-only test.
    #[test]
    fn a_transform_firing_in_every_batch_is_recorded_once() {
        let mut scan = ShapeScan::new();
        for _ in 0..5 {
            let mut b = TableBuilder::new();
            let mut data = ColumnData::F64(vec![f64::NAN, 1.0]);
            crate::canonical::canonicalise(&mut b, "x", &mut data);
            b.push(Column::new("x", "f8"), data).unwrap();
            scan.push(&b.finish()).unwrap();
        }
        let shape = scan.finish();
        let names: Vec<&str> = shape.transforms.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names.len(),
            names.iter().collect::<BTreeSet<_>>().len(),
            "each (transform, column) pair appears once: {names:?}"
        );
        assert!(
            names.contains(&"nan_canonicalisation"),
            "the NaN normalisation is still recorded: {names:?}"
        );
    }

    /// A source whose columns change between chunks is not one table, and the scan says so at the second
    /// batch rather than after reading the whole file.
    #[test]
    fn a_chunk_that_changes_shape_is_rejected_early() {
        let mut scan = ShapeScan::new();
        scan.push(&batch("x", vec![1], None)).unwrap();
        let err = scan.push(&batch("y", vec![2], None)).unwrap_err();
        assert!(
            format!("{err}").contains("changed name between chunks"),
            "got {err}"
        );
    }
}

/// The streaming knobs, beyond the manifest facts [`crate::canonical::GenericIngest`] already carries.
pub struct StreamOpts<'a> {
    /// Staging directory for the write session and the sink's row-group fragments.
    pub stage: &'a std::path::Path,
    /// Where the sealed `.tsra` is written.
    pub out: &'a std::path::Path,
    /// Worker count and RAM budget for the encode pipeline. A **runtime** knob: it must not change a
    /// sealed byte, which the batch-equals-stream test across worker counts is there to prove.
    pub cfg: &'a tessera_io::WriteConfig,
    /// Rows per decoded batch — the read-side bounded-memory unit, independent of the block partition.
    pub batch_rows: usize,
    /// Rows per block. Production passes [`tessera_io::BLOCK_ROWS`]; tests lower it to exercise the
    /// multi-block path without materialising millions of rows.
    pub block_rows: u64,
    /// `--column-meta`: operator semantics that land inside the seal, applied to the same columns the
    /// batch path applies them to.
    pub column_meta: &'a crate::column_meta::ColumnMeta,
}

/// A guard that the input did not change between the two passes.
///
/// Captured when the shape pass opens the file and re-checked before the encode pass, because a two-pass
/// read of a file someone is rewriting would seal a mixture of two inputs under one `ingested_from`
/// digest — silently.
///
/// **What it does not catch**, stated here rather than implied: a modification that preserves size,
/// mtime, ctime and inode. Hashing in-line during the encode pass cannot close that either, because the
/// Parquet reader does random-access reads through a `ChunkReader` and would hash out of order and
/// partially. Single-pass in-line hashing is the only complete answer and the nullability rule forbids
/// it (see the module docs), so this is a metadata guard and is documented as one.
#[derive(Debug, PartialEq, Eq)]
struct InputFingerprint {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    inode: u64,
}

impl InputFingerprint {
    fn of(path: &std::path::Path) -> Result<Self> {
        let md = std::fs::metadata(path).map_err(|e| {
            shape_error(format!("stat {} for the change guard: {e}", path.display()))
        })?;
        Ok(Self {
            len: md.len(),
            modified: md.modified().ok(),
            created: md.created().ok(),
            #[cfg(unix)]
            inode: std::os::unix::fs::MetadataExt::ino(&md),
        })
    }
}

/// Is this input safe to read twice?
///
/// A pipe, fifo or character device cannot be: the second pass would read an exhausted stream and seal a
/// truncated product. Callers fall back to the batch path for these rather than refusing, because
/// `tessera ingest table <(zcat big.csv.gz) …` passes `/dev/fd/63` and works today for CSV — refusing
/// would regress behaviour that exists.
pub fn is_seekable(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|md| md.is_file())
}

/// Two-pass streaming ingest of a re-openable batch source into a sealed `table` product.
///
/// `open` is called **twice** — once for the shape pass and once to encode — which is why it is a
/// closure rather than an iterator. See the module docs for why one pass cannot work.
///
/// Memory is bounded by one decoded batch plus the sink's row-group buffer and the encode pool, never by
/// the file: pass 1 drops each batch after inspecting it, and pass 2 hands each batch to the sink, which
/// spills full row groups to durable fragments under `stage`.
pub fn stream_to_table_product<F, I>(
    open: F,
    exclude: &[String],
    ingest: &crate::canonical::GenericIngest<'_>,
    opts: &StreamOpts<'_>,
) -> Result<tessera_core::Manifest>
where
    F: Fn() -> Result<I>,
    I: Iterator<Item = Result<arrow_array::RecordBatch>>,
{
    let before = InputFingerprint::of(ingest.source_path)?;

    // ── Pass 1: the shape. Every batch is canonicalised with the SAME entry point the batch fold uses,
    //    inspected, and dropped.
    let mut scan = ShapeScan::new();
    for batch in open()? {
        let batch = batch?;
        scan.push(&crate::arrow_table::canonicalise_batch(&batch, exclude)?)?;
    }
    let mut shape = scan.finish();
    if shape.columns.is_empty() {
        return Err(shape_error(format!(
            "{} decoded to no columns",
            ingest.source_path.display()
        )));
    }
    // Operator semantics land inside the seal, on the same columns the batch path applies them to.
    opts.column_meta.apply(&mut shape.columns)?;

    // ── The change guard, between the passes.
    let after = InputFingerprint::of(ingest.source_path)?;
    if before != after {
        return Err(shape_error(format!(
            "{} changed during ingest (size, mtime or inode moved between the shape pass and the \
             encode pass) — re-run on a stable input rather than sealing a mixture of two files",
            ingest.source_path.display()
        )));
    }

    // ── The session. Every manifest fact goes through the shared applier, so this declares exactly what
    //    the batch path declares. The receipt comes from pass 1, which saw every batch, and must be
    //    declared BEFORE any block commits: a streamed `.tsra` is written once with no re-seal hook.
    let mut ws = tessera_io::WriteSession::create(
        opts.stage,
        "table",
        ingest.name,
        ingest.description,
        ingest.timestamp,
    )?;
    crate::canonical::declare_generic_table(&mut ws, ingest, &shape.transforms)?;

    let row_bytes: u64 = shape
        .columns
        .iter()
        .map(|c| {
            u64::try_from(tessera_io::ColumnData::dtype_size(&c.dtype).unwrap_or(0)).unwrap_or(0)
        })
        .sum();
    let unit_bytes = opts.block_rows.saturating_mul(row_bytes.max(1));
    let mut sw = tessera_io::StreamWriter::with_config(ws, opts.cfg, unit_bytes);
    {
        let mut sink = tessera_io::TableMultiBlockSink::with_block_rows(
            shape.columns.clone(),
            crate::canonical::GENERIC_BLOCK,
            &opts.stage.join("sink"),
            &mut sw,
            opts.block_rows,
        )?;
        // ── Pass 2: encode. Each batch is canonicalised, coerced to the file-wide schema so every block
        //    encodes alike, and pushed.
        for batch in open()? {
            let batch = batch?;
            let table = crate::arrow_table::canonicalise_batch(&batch, exclude)?;
            let data = coerce_to_shape(&shape, table)?;
            let named: tessera_io::TableData = shape
                .columns
                .iter()
                .map(|c| c.name.clone())
                .zip(data)
                .collect();
            sink.push(named)?;
        }
        sink.finish()?;
    }
    sw.finish(opts.out)
}
