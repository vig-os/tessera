//! Logical view spanning the `prefix` / `prefix_NNNN` blocks of a partitioned table (ADR-0026 §4).
//!
//! A logical table is the **ordered concatenation** of its per-block columns. This module composes
//! the existing primitives — [`Reader::block_group`] for the manifest-order block list, the
//! per-block [`TableSpec`] carried in each [`BlockRef.spec`] for row counts (no block read), and
//! [`crate::table::decode_column`] / [`crate::table::decode`] for the per-block payload reads —
//! into a unified read/query surface that hides the partition. The reader path the cohort / cloud
//! / query consumers use: full logical column read, lazy per-block stream, random-take over global
//! row indices, and **block-level pruning** before any data read.
//!
//! No bytes are encoded here; this is read-only. Same Reader + same archive → same logical view.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use tessera_core::block::table::{Column, TableSpec};
use tessera_core::chunk_index::{ChunkIndex, ChunkStats};
use tessera_core::{Error, Result};

use crate::chunk_index::cidx_name;
use crate::container::Reader;
use crate::table::{decode, decode_column, ColumnData, TableData};

/// A logical view over the partitioned table named by `prefix`: the ordered concatenation of every
/// block in [`Reader::block_group`]'s result. Cheap to build (manifest-only; no block bytes read)
/// and reusable — methods take a reader by `&mut` so one view can drive many queries.
///
/// **Schema contract.** Every block in the group MUST share the same column count, names, and
/// dtypes — the constructor errors otherwise. The columns surface ([`Self::columns`]) is therefore
/// the single shared schema.
///
/// **Row addressing.** Global rows are `0..row_count()` in manifest order. A global row maps to
/// `(block_idx, local_row)` via the cumulative per-block row counts ([`Self::locate`]). Per-block
/// `TableSpec.rows` comes from each [`BlockRef.spec`] in the manifest, so the addressing is exact
/// without reading any block payload.
pub struct LogicalTableView {
    prefix: String,
    block_names: Vec<String>,
    /// Parallel to [`Self::block_names`]: the per-block [`TableSpec`] decoded from each
    /// [`BlockRef.spec`] (read from the manifest, never from a block payload).
    specs: Vec<TableSpec>,
    /// `cumulative_rows[i]` = sum of `rows` over blocks `0..i` (so `cumulative_rows[block_count()]`
    /// equals [`Self::row_count`]). Drives the global → (block, local) row mapping.
    cumulative_rows: Vec<u64>,
}

impl LogicalTableView {
    /// The block-name prefix this view was built from (e.g. `"events"`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Total logical rows = sum of every block's `rows`.
    pub fn row_count(&self) -> u64 {
        self.cumulative_rows.last().copied().unwrap_or(0)
    }

    /// Number of physical blocks composing this logical table (≥ 1).
    pub fn block_count(&self) -> usize {
        self.block_names.len()
    }

    /// The shared column schema (column order from block 0; constructor enforces every block
    /// matches).
    pub fn columns(&self) -> &[Column] {
        self.specs
            .first()
            .map(|s| s.columns.as_slice())
            .unwrap_or(&[])
    }

    /// The manifest-order block names this view spans (the result of [`Reader::block_group`]).
    pub fn block_names(&self) -> &[String] {
        &self.block_names
    }

    /// Rows in the block at `block_idx` (0-based, < [`Self::block_count`]).
    pub fn block_rows(&self, block_idx: usize) -> Option<u64> {
        self.specs.get(block_idx).map(|s| s.rows)
    }

    /// Map a global row index to its `(block_idx, local_row)` pair via the cumulative row counts.
    /// `Err` when `global_row >= row_count()`.
    pub fn locate(&self, global_row: u64) -> Result<(usize, usize)> {
        let total = self.row_count();
        if global_row >= total {
            return Err(Error::Codec(format!(
                "logical_table('{}'): global row {global_row} >= row_count {total}",
                self.prefix
            )));
        }
        // partition_point returns the first index where the predicate fails — for a sorted
        // cumulative-rows array (strictly non-decreasing) that's the first cumulative beyond
        // `global_row`, and (i - 1) is the block whose [start, end) contains the row.
        let i = self
            .cumulative_rows
            .partition_point(|&c| c <= global_row)
            .saturating_sub(1);
        let local = usize::try_from(global_row - self.cumulative_rows[i])
            .map_err(|e| Error::Codec(format!("logical_table: local row overflows usize: {e}")))?;
        Ok((i, local))
    }

    /// Read the **full logical column**: decode `column` from every block via
    /// [`decode_column`] (Vortex projection — only that column's segments are scanned) and
    /// concatenate in block order. Correctness rests on the format invariant that blocks are
    /// laid out in original row order (ADR-0026 §4).
    ///
    /// For very large columns prefer [`Self::column_blocks`] — it yields one block's worth at a
    /// time so callers can stream the column without materialising it all.
    pub fn column<R: Read + Seek>(&self, reader: &mut Reader<R>, name: &str) -> Result<ColumnData> {
        let code = self.column_dtype(name)?;
        let mut out = ColumnData::from_le_bytes(code, &[])?;
        for (bname, spec) in self.block_names.iter().zip(&self.specs) {
            let blob = reader.read_block(bname)?;
            let chunk = decode_column(spec, &blob, name)?;
            out.extend(&chunk)?;
        }
        Ok(out)
    }

    /// Lazy per-block iterator over `name` — each `next()` reads ONE block, decodes the column via
    /// Vortex projection, and yields it. Bounded memory: one block's column at a time, regardless
    /// of how many blocks the logical table has. The complement of [`Self::column`] (the eager
    /// concatenated form).
    pub fn column_blocks<'a, R: Read + Seek>(
        &'a self,
        reader: &'a mut Reader<R>,
        name: &str,
    ) -> Result<ColumnBlockIter<'a, R>> {
        // Validate up front so the iterator path doesn't need to repeat the schema check per item.
        self.column_dtype(name)?;
        Ok(ColumnBlockIter {
            reader,
            view: self,
            column: name.to_string(),
            idx: 0,
        })
    }

    /// Random-take across blocks: gather the rows named by `global_rows` (in the caller's order)
    /// into a [`TableData`] over the full column schema. Each touched block is decoded **once**
    /// (via [`decode`]), then rows are gathered from the cached decodes — so the work scales with
    /// `len(touched_blocks)` block decodes + `len(global_rows)` row copies, not `len(global_rows)`
    /// block reads.
    ///
    /// **Per-block primitive used.** This composes [`decode`] (the full per-block decode) +
    /// in-memory indexing. The Vortex random-take primitive (`Selection::IncludeByIndex` via
    /// `with_row_indices`) is not exposed through [`crate::table`] today; switching to it for the
    /// per-block step is a future optimisation (would replace `decode` + index with a projected
    /// scan that materialises only the requested rows — same outputs, less work for small selects).
    pub fn take<R: Read + Seek>(
        &self,
        reader: &mut Reader<R>,
        global_rows: &[u64],
    ) -> Result<TableData> {
        let columns = self.columns();
        // Map every requested global row to (block_idx, local_row) in caller order.
        let mut mapping: Vec<(usize, usize)> = Vec::with_capacity(global_rows.len());
        for &g in global_rows {
            mapping.push(self.locate(g)?);
        }
        // Decode each touched block exactly once (BTreeMap keeps the keyed cache simple).
        let mut decoded: BTreeMap<usize, TableData> = BTreeMap::new();
        for &(b, _) in &mapping {
            if let std::collections::btree_map::Entry::Vacant(e) = decoded.entry(b) {
                let blob = reader.read_block(&self.block_names[b])?;
                e.insert(decode(&self.specs[b], &blob)?);
            }
        }
        // Build output columns in spec order; gather one row at a time in caller order.
        let mut out: TableData = Vec::with_capacity(columns.len());
        for (col_idx, col) in columns.iter().enumerate() {
            let mut typed = ColumnData::from_le_bytes(&col.dtype, &[])?;
            for &(b, l) in &mapping {
                let src = decoded.get(&b).ok_or_else(|| {
                    Error::Codec(format!("take: missing decoded cache for block {b}"))
                })?;
                let cell = src
                    .get(col_idx)
                    .ok_or_else(|| {
                        Error::Codec(format!(
                            "take: block {b} missing column index {col_idx} ('{}')",
                            col.name
                        ))
                    })?
                    .1
                    .slice(l, l + 1);
                typed.extend(&cell)?;
            }
            out.push((col.name.clone(), typed));
        }
        Ok(out)
    }

    /// **Block-level pruning** — return the indices of blocks whose `[min, max]` for `column`
    /// overlaps the inclusive integer range `[lo, hi]`. Blocks outside the range are provably
    /// skippable; the result is the set of blocks a ranged query MUST read.
    ///
    /// **Where the stats come from.**
    /// - If a `<block>.cidx` sidecar (ADR-0028 §3) exists in the manifest AND the requested
    ///   `column` matches that block's `TableSpec.row_index` (the conventional cidx stat column —
    ///   the sidecar payload itself records no column name today), the sidecar is read and its
    ///   rolled-up `aggregate()` supplies the `[min, max]`. **No data-block bytes are touched**
    ///   in that path — the prune-before-fetch primitive cohort/cloud reads will use.
    /// - Otherwise the stat column is read via [`decode_column`] (Vortex projection — only that
    ///   column's segments) and folded into [`ChunkStats`] directly. Still cheaper than a full
    ///   decode, but does cost one column-projection scan per block.
    ///
    /// `lo <= hi` is required (debug-asserted via [`ChunkStats::overlaps`]'s contract); the column
    /// MUST be an integer column on the fallback path (floats need ADR-0024 canonicalisation
    /// before integer stats are meaningful).
    pub fn select_blocks_overlapping<R: Read + Seek>(
        &self,
        reader: &mut Reader<R>,
        column: &str,
        lo: i64,
        hi: i64,
    ) -> Result<Vec<usize>> {
        let mut out = Vec::new();
        for b in 0..self.block_names.len() {
            let stats = self.block_stats(reader, b, column)?;
            if stats.overlaps(lo, hi) {
                out.push(b);
            }
        }
        Ok(out)
    }

    fn column_dtype(&self, name: &str) -> Result<&str> {
        self.specs
            .first()
            .and_then(|s| s.columns.iter().find(|c| c.name == name))
            .map(|c| c.dtype.as_str())
            .ok_or_else(|| {
                Error::Codec(format!(
                    "logical_table('{}'): no column '{name}'",
                    self.prefix
                ))
            })
    }

    fn block_stats<R: Read + Seek>(
        &self,
        reader: &mut Reader<R>,
        block_idx: usize,
        column: &str,
    ) -> Result<ChunkStats> {
        let bname = &self.block_names[block_idx];
        let spec = &self.specs[block_idx];
        // Prefer the sidecar when (a) it exists in the manifest, and (b) the requested column is
        // the block's `row_index` — the conventional stat column the writer feeds to
        // `table_block_with_index`. The sidecar payload itself doesn't record which column its
        // stats are over, so we trust this heuristic rather than reading the wrong stats.
        let sidecar = cidx_name(bname);
        let sidecar_present = reader.manifest().blocks.iter().any(|b| b.name == sidecar);
        let is_row_index = spec.row_index.as_deref() == Some(column);
        if sidecar_present && is_row_index {
            let bytes = reader.read_block(&sidecar)?;
            return Ok(ChunkIndex::from_bytes(&bytes)?.aggregate());
        }
        // Fallback: project just the stat column (Vortex column projection is cheap; we never
        // decode the whole block to learn its min/max).
        let blob = reader.read_block(bname)?;
        let col = decode_column(spec, &blob, column)?;
        let vals = col.as_i64().ok_or_else(|| {
            Error::Codec(format!(
                "select_blocks_overlapping: column '{column}' is not an integer column for stats"
            ))
        })?;
        Ok(ChunkStats::from_values(&vals))
    }
}

/// Lazy per-block iterator returned by [`LogicalTableView::column_blocks`]. Each `next()` reads
/// one block's payload and decodes only the requested column (Vortex projection), so memory stays
/// bounded at ~one block's column-bytes regardless of how many blocks the table holds.
///
/// After an error item, the iterator fuses (returns `None` on subsequent calls) — partial-read
/// observers MUST honour the error rather than continuing past it.
pub struct ColumnBlockIter<'a, R: Read + Seek> {
    reader: &'a mut Reader<R>,
    view: &'a LogicalTableView,
    column: String,
    idx: usize,
}

impl<'a, R: Read + Seek> Iterator for ColumnBlockIter<'a, R> {
    type Item = Result<ColumnData>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.view.block_names.len() {
            return None;
        }
        let bname = &self.view.block_names[self.idx];
        let spec = &self.view.specs[self.idx];
        self.idx += 1;
        let blob = match self.reader.read_block(bname) {
            Ok(b) => b,
            Err(e) => {
                // Fuse: don't try further blocks after an error.
                self.idx = self.view.block_names.len();
                return Some(Err(e));
            }
        };
        let decoded = decode_column(spec, &blob, &self.column);
        if decoded.is_err() {
            // Fuse on a decode error too (matching the read_block arm) — a column that fails to
            // decode in one block won't succeed in later ones, and the doc promises fusing.
            self.idx = self.view.block_names.len();
        }
        Some(decoded)
    }
}

impl<R: Read + Seek> Reader<R> {
    /// Build a [`LogicalTableView`] over the partitioned table named by `prefix` — the ordered
    /// concatenation of every `prefix` / `prefix_NNNN` block returned by [`Self::block_group`].
    /// Cheap (manifest-only; no block bytes read): parses each block's [`TableSpec`] from its
    /// recorded [`BlockRef.spec`] to learn per-block row counts and verifies every block shares
    /// the same column schema (count + names + dtypes).
    ///
    /// Errors if `prefix` matches no blocks in the manifest, or if the matched blocks disagree on
    /// their schema (a corrupt or hand-built product).
    pub fn logical_table(&mut self, prefix: &str) -> Result<LogicalTableView> {
        let block_names = self.block_group(prefix);
        if block_names.is_empty() {
            return Err(Error::Container(format!(
                "logical_table: no blocks for prefix '{prefix}'"
            )));
        }
        let mut specs: Vec<TableSpec> = Vec::with_capacity(block_names.len());
        for name in &block_names {
            let br = self
                .manifest()
                .blocks
                .iter()
                .find(|b| &b.name == name)
                .ok_or_else(|| {
                    Error::Container(format!("logical_table: missing BlockRef for '{name}'"))
                })?;
            let spec: TableSpec = serde_json::from_value(br.spec.clone())?;
            specs.push(spec);
        }
        // Schema consistency: every block MUST agree on column count, names, and dtypes. The
        // logical column would otherwise be ill-defined (which dtype to concat under?).
        let head = specs[0].clone();
        for (i, s) in specs.iter().enumerate().skip(1) {
            if s.columns.len() != head.columns.len() {
                return Err(Error::Container(format!(
                    "logical_table('{prefix}'): block {i} has {} columns, block 0 has {}",
                    s.columns.len(),
                    head.columns.len()
                )));
            }
            for (j, (a, b)) in s.columns.iter().zip(&head.columns).enumerate() {
                if a.name != b.name || a.dtype != b.dtype {
                    return Err(Error::Container(format!(
                        "logical_table('{prefix}'): block {i} column {j} \
                         ({:?}/{:?}) != block 0 ({:?}/{:?})",
                        a.name, a.dtype, b.name, b.dtype
                    )));
                }
            }
        }
        // Cumulative row offsets — drives the O(log block_count) global → (block, local) lookup.
        let mut cumulative_rows: Vec<u64> = Vec::with_capacity(block_names.len() + 1);
        cumulative_rows.push(0);
        let mut acc: u64 = 0;
        for s in &specs {
            acc = acc.checked_add(s.rows).ok_or_else(|| {
                Error::Container("logical_table: total row count overflows u64".into())
            })?;
            cumulative_rows.push(acc);
        }
        Ok(LogicalTableView {
            prefix: prefix.to_string(),
            block_names,
            specs,
            cumulative_rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accumulate::TableMultiBlockSink;
    use crate::stream::StreamWriter;
    use crate::table::{self, ROWS_PER_GROUP};
    use crate::write::WriteSession;
    use tessera_core::block::table::Column;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn col(name: &str, dtype: &str) -> Column {
        Column {
            name: name.into(),
            dtype: dtype.into(),
            codec: None,
        }
    }

    /// Build a sealed multi-block listmode-style product whose `t` (u64) column is the monotonic
    /// global row index (so prune semantics are exact) and `e` (f32) is a deterministic pattern.
    /// Uses the test seam `TableMultiBlockSink::with_block_rows` to partition at a SMALL
    /// `block_rows` so we don't have to materialise the production BLOCK_ROWS (~4M rows).
    fn build_multi_block(
        dir: &std::path::Path,
        rows: usize,
        block_rows: u64,
    ) -> std::path::PathBuf {
        let columns = vec![col("t", "u8"), col("e", "f4")];
        let ws = WriteSession::create(&dir.join("ws"), "listmode", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 2, 4);
        {
            let mut sink = TableMultiBlockSink::with_block_rows(
                columns,
                "events",
                &dir.join("sink"),
                &mut sw,
                block_rows,
            )
            .unwrap()
            .with_row_index("t");
            // push in a misaligned batch to exercise the cross-grid path the real DAQ hits.
            let batch: TableData = vec![
                ("t".into(), ColumnData::U64((0..rows as u64).collect())),
                (
                    "e".into(),
                    ColumnData::F32((0..rows).map(|k| 511.0 + (k % 13) as f32).collect()),
                ),
            ];
            sink.push(batch).unwrap();
            sink.finish().unwrap();
        }
        let out = dir.join("multi.tsra");
        sw.finish(&out).unwrap();
        out
    }

    #[test]
    fn logical_row_count_is_sum_of_blocks() {
        // 3 blocks at the test seam — and the single-block view (rows ≤ block_rows) behaves
        // identically: row_count == sum of block specs and is what every other method sees.
        let dir = tempfile::tempdir().unwrap();
        let block_rows = ROWS_PER_GROUP as u64; // 1 row-group per block → fast multi-block test
        let rows = (block_rows as usize) * 2 + 4242; // 3 blocks (2 full + remainder)
        let path = build_multi_block(dir.path(), rows, block_rows);
        let mut rdr = Reader::open(&path).unwrap();
        let view = rdr.logical_table("events").unwrap();
        assert_eq!(view.block_count(), 3);
        assert_eq!(view.row_count(), rows as u64);
        // Per-block row counts agree with the partition (full blocks + trailing remainder).
        assert_eq!(view.block_rows(0), Some(block_rows));
        assert_eq!(view.block_rows(1), Some(block_rows));
        assert_eq!(view.block_rows(2), Some(rows as u64 - 2 * block_rows));
        // Shared schema surfaces from block 0 — both columns present in the documented order.
        let cols: Vec<&str> = view.columns().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cols, vec!["t", "e"]);
    }

    #[test]
    fn logical_single_block_view_matches_one_block_table() {
        // The corpus-safety invariant on the read side: a product with rows ≤ block_rows yields
        // exactly one block named `events`, and the logical view over it equals the single-block
        // read path. So small products keep their pre-partition reader semantics.
        let dir = tempfile::tempdir().unwrap();
        let rows = 5_000usize; // well under ROWS_PER_GROUP → exactly one `events` block
        let path = build_multi_block(dir.path(), rows, ROWS_PER_GROUP as u64);
        let mut rdr = Reader::open(&path).unwrap();
        let names = rdr.block_group("events");
        assert_eq!(names, vec!["events"]); // small-stays-single
        let view = rdr.logical_table("events").unwrap();
        assert_eq!(view.block_count(), 1);
        assert_eq!(view.row_count(), rows as u64);
        // Reading via the logical view == reading the single block directly.
        let logical_t = view.column(&mut rdr, "t").unwrap();
        let blob = rdr.read_block("events").unwrap();
        let spec: TableSpec =
            serde_json::from_value(rdr.manifest().blocks[0].spec.clone()).unwrap();
        let direct_t = table::decode_column(&spec, &blob, "t").unwrap();
        assert_eq!(logical_t, direct_t);
    }

    #[test]
    fn logical_column_equals_concatenation_of_per_block_decode() {
        // The defining property: view.column(name) == per-block decode_column concatenated in
        // manifest order. Covers both `t` (u64, sorted) and `e` (f32, deterministic pattern).
        let dir = tempfile::tempdir().unwrap();
        let block_rows = ROWS_PER_GROUP as u64;
        let rows = (block_rows as usize) * 2 + 1234;
        let path = build_multi_block(dir.path(), rows, block_rows);
        let mut rdr = Reader::open(&path).unwrap();
        let view = rdr.logical_table("events").unwrap();
        let names = view.block_names().to_vec();
        let specs: Vec<TableSpec> = names
            .iter()
            .map(|n| {
                let br = rdr
                    .manifest()
                    .blocks
                    .iter()
                    .find(|b| &b.name == n)
                    .unwrap()
                    .clone();
                serde_json::from_value(br.spec).unwrap()
            })
            .collect();

        for column in ["t", "e"] {
            // Per-block decode_column path — the "decompose then verify" baseline. Seed with the
            // matching spec column's dtype so the accumulator is the right typed variant.
            let code = &specs[0]
                .columns
                .iter()
                .find(|c| c.name == column)
                .unwrap()
                .dtype;
            let mut expected = ColumnData::from_le_bytes(code, &[]).unwrap();
            for (n, s) in names.iter().zip(&specs) {
                let blob = rdr.read_block(n).unwrap();
                let chunk = table::decode_column(s, &blob, column).unwrap();
                expected.extend(&chunk).unwrap();
            }
            // Logical view path — what the LogicalTableView::column delivers.
            let actual = view.column(&mut rdr, column).unwrap();
            assert_eq!(
                actual, expected,
                "logical column '{column}' != concatenated per-block decode_column"
            );
            assert_eq!(actual.len(), rows);
        }

        // And the lazy per-block iterator returns the same per-block columns in order.
        let lazy: Vec<ColumnData> = view
            .column_blocks(&mut rdr, "t")
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(lazy.len(), view.block_count());
        let lazy_total: usize = lazy.iter().map(|c| c.len()).sum();
        assert_eq!(lazy_total, rows);
    }

    #[test]
    fn take_returns_requested_rows_in_order_across_block_boundaries() {
        // Pick global row indices that straddle ≥ 2 block boundaries and ask for them in a
        // non-monotonic order — the take MUST return the values in the caller's order, not the
        // gathered order (the difference between a random-take API and a filter).
        let dir = tempfile::tempdir().unwrap();
        let block_rows = ROWS_PER_GROUP as u64;
        let rows = (block_rows as usize) * 2 + 777;
        let path = build_multi_block(dir.path(), rows, block_rows);
        let mut rdr = Reader::open(&path).unwrap();
        let view = rdr.logical_table("events").unwrap();
        assert!(
            view.block_count() >= 3,
            "need ≥ 3 blocks to span boundaries"
        );

        // Hand-picked indices: one in block 0, one in block 1, one in block 2, then doubles +
        // boundary cells (the last row of block 0 and the first row of block 1).
        let last_b0 = block_rows - 1;
        let first_b1 = block_rows;
        let mid_b1 = block_rows + (block_rows / 2);
        let in_b2 = 2 * block_rows + 5;
        let global_rows = vec![in_b2, last_b0, first_b1, 0, mid_b1, last_b0];
        let out = view.take(&mut rdr, &global_rows).unwrap();

        // Column `t` IS the global row index by construction, so the returned `t` MUST equal the
        // caller's requested order — the strongest possible check for cross-block correctness.
        let ColumnData::U64(t) = &out
            .iter()
            .find(|(n, _)| n == "t")
            .expect("output keeps the 't' column")
            .1
        else {
            panic!("'t' must decode as U64")
        };
        assert_eq!(t.as_slice(), global_rows.as_slice());

        // And `e` follows the deterministic pattern at those same row indices.
        let ColumnData::F32(e) = &out
            .iter()
            .find(|(n, _)| n == "e")
            .expect("output keeps the 'e' column")
            .1
        else {
            panic!("'e' must decode as F32")
        };
        let expected_e: Vec<f32> = global_rows
            .iter()
            .map(|&g| 511.0 + ((g as usize) % 13) as f32)
            .collect();
        assert_eq!(e.as_slice(), expected_e.as_slice());

        // An out-of-bounds global row is a typed Codec error, not a panic.
        let bad = view.take(&mut rdr, &[view.row_count()]);
        assert!(matches!(bad, Err(Error::Codec(_))));
    }

    #[test]
    fn select_blocks_overlapping_skips_non_matching_blocks() {
        // `t` is the global row index → monotonic across blocks → exact pruning: a range that
        // falls inside one block MUST return exactly that block's index, and a range outside the
        // whole table MUST return an empty vec.
        let dir = tempfile::tempdir().unwrap();
        let block_rows = ROWS_PER_GROUP as u64;
        let rows = (block_rows as usize) * 3; // exactly 3 full blocks
        let path = build_multi_block(dir.path(), rows, block_rows);
        let mut rdr = Reader::open(&path).unwrap();
        let view = rdr.logical_table("events").unwrap();
        assert_eq!(view.block_count(), 3);

        // A range fully inside block 1 → only block 1.
        let lo = block_rows as i64 + 10;
        let hi = block_rows as i64 + 20;
        assert_eq!(
            view.select_blocks_overlapping(&mut rdr, "t", lo, hi)
                .unwrap(),
            vec![1]
        );

        // A range spanning blocks 0 and 1 (boundary inclusive) → blocks 0 and 1.
        let lo = block_rows as i64 - 5;
        let hi = block_rows as i64 + 5;
        assert_eq!(
            view.select_blocks_overlapping(&mut rdr, "t", lo, hi)
                .unwrap(),
            vec![0, 1]
        );

        // A range beyond every value → no blocks.
        let lo = view.row_count() as i64 + 1;
        let hi = lo + 100;
        assert_eq!(
            view.select_blocks_overlapping(&mut rdr, "t", lo, hi)
                .unwrap(),
            Vec::<usize>::new()
        );

        // A range that covers the whole monotonic column → every block.
        let lo = 0;
        let hi = view.row_count() as i64 - 1;
        assert_eq!(
            view.select_blocks_overlapping(&mut rdr, "t", lo, hi)
                .unwrap(),
            vec![0, 1, 2]
        );

        // Float columns can't supply integer min/max (ADR-0024) — the fallback path errors.
        assert!(view
            .select_blocks_overlapping(&mut rdr, "e", 0, 100)
            .is_err());
    }

    #[test]
    fn logical_table_errors_on_unknown_prefix() {
        // An empty block_group MUST be a typed Container error, not a silent empty view (callers
        // distinguish "no such table" from "empty table" via this path).
        let dir = tempfile::tempdir().unwrap();
        let path = build_multi_block(dir.path(), 4_000, ROWS_PER_GROUP as u64);
        let mut rdr = Reader::open(&path).unwrap();
        assert!(matches!(
            rdr.logical_table("missing"),
            Err(Error::Container(_))
        ));
    }
}
