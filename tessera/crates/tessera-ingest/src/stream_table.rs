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
