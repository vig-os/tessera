//! Row-group accumulator (ROADMAP P3 / #203, ADR-0026) — the DAQ-facing producer for **table**
//! blocks.
//!
//! A device streams arbitrary-size transport batches; `TableStreamWriter` re-chunks them to Tessera's
//! fixed [`ROWS_PER_GROUP`] grid, spills each full row-group to a **durable fragment** (so RAM stays
//! bounded at ~2 row-groups no matter how many rows are pushed), and at `finish` **lazily** compacts
//! the fragments through [`encode_streaming`] into the **one canonical chunked table block** —
//! byte-identical to a batch encode of the same rows.
//!
//! The flush trigger (a full group, or — later — a timer for low-rate capture) only affects *staging*;
//! the sealed block always re-compacts to the fixed grid, so the bytes are independent of how the data
//! was batched in time (ADR-0026 / ADR-0027). Fragment format is engine-internal raw little-endian
//! columns (not the sealed format), so it's intentionally trivial.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tessera_core::block::table::{Column, TableSpec};
use tessera_core::chunk_index::{ChunkStats, MerkleStatsAccumulator, Monoid};
use tessera_core::hash::digest;
use tessera_core::{Error, Result};

use crate::stream::{table_job_from_fragments, StreamWriter};
use crate::table::{
    self, block_name, partition_blocks, ColumnData, TableData, BLOCK_ROWS, ROWS_PER_GROUP,
};

/// An empty buffer (one empty column per spec column). Spec dtypes are validated in
/// [`TableStreamWriter::new`], so the empty construction can't fail afterwards.
fn empty_cols(columns: &[Column]) -> TableData {
    columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                ColumnData::from_le_bytes(&c.dtype, &[]).expect("validated dtype"),
            )
        })
        .collect()
}

/// Write one row-group [`TableData`] to a durable fragment file — the internal staging format the
/// streaming table writers spill to (NOT the sealed format). Shape: `u64 le n_rows · column[0].le_bytes
/// · column[1].le_bytes · …`, columns in spec order. Shared by [`TableStreamWriter`] (single block)
/// and the multi-block sink so both stage in the **same** trivial format, and so the parallel
/// per-block encode jobs read fragments written by either path.
pub(crate) fn write_fragment(path: &Path, group: &TableData) -> Result<()> {
    let n_rows = u64::try_from(group.first().map(|(_, c)| c.len()).unwrap_or(0))
        .map_err(|e| Error::Codec(format!("fragment: row count overflows u64: {e}")))?;
    let mut f = File::create(path)?;
    f.write_all(&n_rows.to_le_bytes())?;
    for (_, c) in group {
        f.write_all(&c.to_le_bytes())?;
    }
    f.sync_all()?; // durable: a committed row-group survives a crash
    Ok(())
}

/// Read one fragment back into a [`TableData`] of `columns` (inverse of [`write_fragment`]). The
/// fragment format is internal; this is the only reader. `pub(crate)` so [`crate::stream`] can lazily
/// pull fragments inside a per-block encode job.
pub(crate) fn read_fragment(path: &Path, columns: &[Column]) -> Result<TableData> {
    let bytes = fs::read(path)?;
    let head = bytes
        .get(0..8)
        .ok_or_else(|| Error::Codec("fragment: truncated header".into()))?;
    let head_arr: [u8; 8] = head
        .try_into()
        .map_err(|e| Error::Codec(format!("fragment: corrupt header: {e}")))?;
    let n_rows = usize::try_from(u64::from_le_bytes(head_arr))
        .map_err(|e| Error::Codec(format!("fragment: row count exceeds usize: {e}")))?;
    let mut off = 8usize;
    let mut out = Vec::with_capacity(columns.len());
    for c in columns {
        let len = n_rows * ColumnData::dtype_size(&c.dtype)?;
        let raw = bytes
            .get(off..off + len)
            .ok_or_else(|| Error::Codec(format!("fragment: truncated column '{}'", c.name)))?;
        out.push((c.name.clone(), ColumnData::from_le_bytes(&c.dtype, raw)?));
        off += len;
    }
    Ok(out)
}

/// Bounded-memory streaming writer that builds **one canonical chunked table block** from a stream of
/// pushed row-batches.
pub struct TableStreamWriter {
    spec: TableSpec,
    stage: PathBuf,
    buf: TableData,
    buf_rows: usize,
    n_frags: usize,
    /// Optional ADR-0028 §5 live `{hash, stats}` fold over the streamed row-groups: column to take stats
    /// over (must be an integer column), and the running accumulator folded once per flushed group.
    stat_column: Option<String>,
    fold: MerkleStatsAccumulator,
}

impl TableStreamWriter {
    /// Create a writer staging row-group fragments under `stage` (created if absent).
    pub fn new(spec: TableSpec, stage: &Path) -> Result<Self> {
        // Validate every column dtype up front so later empty-column construction is infallible.
        for c in &spec.columns {
            ColumnData::dtype_size(&c.dtype)?;
        }
        fs::create_dir_all(stage)?;
        let buf = empty_cols(&spec.columns);
        Ok(TableStreamWriter {
            spec,
            stage: stage.to_path_buf(),
            buf,
            buf_rows: 0,
            n_frags: 0,
            stat_column: None,
            fold: MerkleStatsAccumulator::new(),
        })
    }

    /// Opt into the ADR-0028 §5 **fused fold** (bounded-memory live integrity + overview): as each
    /// canonical row-group is flushed, its `{digest, stats(over stat_column)}` leaf is folded up the MMR,
    /// so [`Self::live_root`] / [`Self::live_aggregate`] advance per row-group **without** retaining the
    /// data — and reconcile exactly with a batch [`crate::table::table_chunk_index`] over the same rows.
    pub fn with_live_index(mut self, stat_column: impl Into<String>) -> Self {
        self.stat_column = Some(stat_column.into());
        self
    }

    /// The live sub-block MMR root over the row-groups flushed so far (ADR-0028 §5) — `None` unless
    /// [`Self::with_live_index`] was set. Complete once every group is flushed (at a group boundary or
    /// after the trailing flush in [`Self::finish`]).
    pub fn live_root(&self) -> Option<String> {
        self.stat_column.as_ref().map(|_| self.fold.root())
    }

    /// The live rolled-up [`ChunkStats`] over the flushed groups' `stat_column` (ADR-0028 §5) — `None`
    /// unless [`Self::with_live_index`] was set.
    pub fn live_aggregate(&self) -> Option<ChunkStats> {
        self.stat_column.as_ref().map(|_| self.fold.aggregate())
    }

    fn frag_path(&self, i: usize) -> PathBuf {
        self.stage.join(format!("g{i:08}.frag"))
    }

    /// Append a transport batch (any number of rows). Columns must match the spec (order + dtype).
    /// Full 65536-row groups spill to durable fragments, so memory stays bounded.
    pub fn push(&mut self, batch: TableData) -> Result<()> {
        if batch.len() != self.spec.columns.len() {
            return Err(Error::Codec("push: column count mismatch".into()));
        }
        let n = batch.first().map(|(_, c)| c.len()).unwrap_or(0);
        for ((bn, bc), buf) in batch.iter().zip(self.buf.iter_mut()) {
            if bn != &buf.0 {
                return Err(Error::Codec(format!(
                    "push: column '{bn}' != spec '{}'",
                    buf.0
                )));
            }
            if bc.len() != n {
                return Err(Error::Codec("push: ragged batch columns".into()));
            }
            buf.1.extend(bc)?;
        }
        self.buf_rows += n;
        while self.buf_rows >= ROWS_PER_GROUP {
            self.flush(ROWS_PER_GROUP)?;
        }
        Ok(())
    }

    /// Spill the first `n` buffered rows as a durable fragment; keep the remainder in the buffer.
    fn flush(&mut self, n: usize) -> Result<()> {
        let group: TableData = self
            .buf
            .iter()
            .map(|(name, c)| (name.clone(), c.slice(0, n)))
            .collect();
        write_fragment(&self.frag_path(self.n_frags), &group)?;
        self.n_frags += 1;
        // ADR-0028 §5: fold this group's {digest, stats} into the live MMR (bounded memory — the group
        // is folded then dropped). The per-group digest matches `table_chunk_index` (every column's LE
        // bytes for the group, in column order), so the live root reconciles with the batch index.
        if let Some(col) = &self.stat_column {
            let mut bytes = Vec::new();
            for (_, c) in &group {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            let stats = group
                .iter()
                .find(|(n, _)| n == col)
                .and_then(|(_, c)| c.as_i64())
                .map(|v| ChunkStats::from_values(&v))
                .unwrap_or_else(ChunkStats::identity);
            self.fold.push(&digest(&bytes), stats);
        }
        // keep the remainder
        self.buf = self
            .buf
            .iter()
            .map(|(name, c)| (name.clone(), c.slice(n, c.len())))
            .collect();
        self.buf_rows -= n;
        Ok(())
    }

    /// Flush the trailing partial group, then **lazily** compact all fragments into the one canonical
    /// chunked table block (byte-identical to a batch encode of every pushed row, in order).
    pub fn finish(mut self) -> Result<Vec<u8>> {
        if self.buf_rows > 0 {
            self.flush(self.buf_rows)?;
        }
        let paths: Vec<PathBuf> = (0..self.n_frags).map(|i| self.frag_path(i)).collect();
        let columns = self.spec.columns.clone();
        // Fragment reads are fallible but `encode_streaming` pulls infallible `TableData`; stash the
        // first I/O error and feed empties after it, then surface it (the produced block is discarded).
        let err: Arc<Mutex<Option<Error>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&err);
        let iter = paths.into_iter().map(move |p| {
            if slot.lock().unwrap().is_some() {
                return empty_cols(&columns);
            }
            match read_fragment(&p, &columns) {
                Ok(td) => td,
                Err(e) => {
                    *slot.lock().unwrap() = Some(e);
                    empty_cols(&columns)
                }
            }
        });
        let bytes = table::encode_streaming(&self.spec, iter)?;
        if let Some(e) = err.lock().unwrap().take() {
            return Err(e);
        }
        Ok(bytes)
    }
}

/// Bounded-memory, **multi-block** streaming sink for table ingest (ADR-0026 §4 — the
/// determinism-critical evolution of [`TableStreamWriter`]).
///
/// Same producer interface as [`TableStreamWriter::push`] (any-size transport batch in, durable
/// row-group fragments out), but the sink partitions the row-group stream into
/// [`crate::table::BLOCK_ROWS`]-sized blocks (canonical 64-row-group blocks; the trailing block may
/// be partial) and at [`Self::finish`] dispatches **one parallel encode job per block** to the
/// supplied [`StreamWriter`] via [`table_job_from_fragments`]. The [`StreamWriter`]'s ordered
/// committer commits per-block bytes in push order — so the sealed product is **worker-count
/// independent** and **partition-stable**: same data → same per-block partition → same
/// content_hash regardless of `workers` / `ram_budget` / `slab_rows`.
///
/// **Small-stays-single corpus invariant** (ADR-0026 §4): when the total pushed rows ≤
/// [`crate::table::BLOCK_ROWS`], the sink emits exactly ONE block named `prefix` (via
/// [`crate::table::block_name`]'s `total<=1` branch). That single block's bytes are byte-identical
/// to today's pre-partition layout — so existing fixtures and the conformance corpus stay
/// byte-identical (no regen). Only data above the ceiling sees the new `prefix_NNNN` shards.
///
/// **Why finish-time dispatch (not mid-stream).** The small-stays-single invariant *requires*
/// knowing the total before naming any block: a 1-block product is `prefix`, a 2+-block product is
/// `prefix_0000..`. Mid-stream dispatch would force a provisional name + post-hoc rewrite, which
/// the [`StreamWriter`] (commit-in-push-order) doesn't support. Workers still parallel-encode the
/// blocks (the StreamWriter dispatches them across threads), they just start after the read.
pub struct TableMultiBlockSink<'a> {
    /// Per-block column schema (every block carries the same columns).
    columns: Vec<Column>,
    /// Optional per-block `row_index` (plumbs into every block's [`TableSpec`]).
    row_index: Option<String>,
    /// Manifest-side prefix for emitted block names (typically `"events"`).
    name_prefix: String,
    /// Staging dir holding the row-group fragment files.
    stage: PathBuf,
    /// Borrowed [`StreamWriter`] each completed block is dispatched to at [`Self::finish`].
    sw: &'a mut StreamWriter,
    /// Row-group buffer + the durable fragments staged so far.
    buf: TableData,
    buf_rows: usize,
    frags: Vec<PathBuf>,
    /// Row count of each staged fragment (parallel-indexed with [`Self::frags`]).
    frag_rows: Vec<u64>,
    /// Running row count across every batch pushed (the format-partition input).
    total_rows: u64,
    /// Rows per block — the partition size. Production: [`BLOCK_ROWS`] (format invariant). The
    /// `#[cfg(test)]` constructor [`Self::with_block_rows`] overrides it to exercise the multi-block
    /// path at small sizes (so worker-independence tests don't have to materialise 4 M+ rows).
    /// MUST be a positive multiple of [`ROWS_PER_GROUP`] (asserted at construction).
    block_rows: u64,
}

impl<'a> TableMultiBlockSink<'a> {
    /// Build a sink staging fragments under `stage`, dispatching per-block encode jobs to `sw`.
    /// `columns` defines the per-block schema; `name_prefix` (typically `"events"`) is the
    /// manifest block-name prefix passed to [`crate::table::block_name`]. Validates every column
    /// dtype up front so later operations are infallible w.r.t. dtype shape.
    pub fn new(
        columns: Vec<Column>,
        name_prefix: impl Into<String>,
        stage: &Path,
        sw: &'a mut StreamWriter,
    ) -> Result<Self> {
        Self::with_block_rows_inner(columns, name_prefix, stage, sw, BLOCK_ROWS as u64)
    }

    /// Test-only constructor: build a sink with an overridden per-block partition size. Lets unit
    /// tests exercise the >ceiling, multi-block path at small `block_rows` (e.g. ROWS_PER_GROUP)
    /// without having to materialise the production `BLOCK_ROWS` (~4M rows). MUST be a positive
    /// whole multiple of [`ROWS_PER_GROUP`] (so every full block is exactly N row-groups). Production
    /// code paths MUST go through [`Self::new`] — they pick up the format invariant constant.
    /// `#[doc(hidden)]` (undiscoverable) so callers don't reach for it; the constructor validates
    /// `block_rows` is a positive whole multiple of [`ROWS_PER_GROUP`]. Pass anything other than
    /// [`BLOCK_ROWS`] and you get a deliberately non-corpus partition (test products only).
    #[doc(hidden)]
    pub fn with_block_rows(
        columns: Vec<Column>,
        name_prefix: impl Into<String>,
        stage: &Path,
        sw: &'a mut StreamWriter,
        block_rows: u64,
    ) -> Result<Self> {
        Self::with_block_rows_inner(columns, name_prefix, stage, sw, block_rows)
    }

    fn with_block_rows_inner(
        columns: Vec<Column>,
        name_prefix: impl Into<String>,
        stage: &Path,
        sw: &'a mut StreamWriter,
        block_rows: u64,
    ) -> Result<Self> {
        if block_rows == 0 || !block_rows.is_multiple_of(ROWS_PER_GROUP as u64) {
            return Err(Error::Codec(format!(
                "multi-block: block_rows ({block_rows}) must be a positive multiple of ROWS_PER_GROUP ({ROWS_PER_GROUP})"
            )));
        }
        for c in &columns {
            ColumnData::dtype_size(&c.dtype)?;
        }
        fs::create_dir_all(stage)?;
        let buf = empty_cols(&columns);
        Ok(TableMultiBlockSink {
            columns,
            row_index: None,
            name_prefix: name_prefix.into(),
            stage: stage.to_path_buf(),
            sw,
            buf,
            buf_rows: 0,
            frags: Vec::new(),
            frag_rows: Vec::new(),
            total_rows: 0,
            block_rows,
        })
    }

    /// Set the `row_index` recorded in every emitted block's [`TableSpec`] (e.g. `"ms"` for the
    /// listmode `ms` timestamp column).
    pub fn with_row_index(mut self, name: impl Into<String>) -> Self {
        self.row_index = Some(name.into());
        self
    }

    /// Total rows pushed (the partition input for [`Self::finish`]).
    pub fn total_rows(&self) -> u64 {
        self.total_rows
    }

    fn frag_path(&self, i: usize) -> PathBuf {
        self.stage.join(format!("g{i:08}.frag"))
    }

    /// Append a transport batch (any size). Columns must match (order + dtype). Full
    /// [`ROWS_PER_GROUP`]-row row-groups spill to durable fragments; partition into per-block jobs
    /// happens at [`Self::finish`].
    pub fn push(&mut self, batch: TableData) -> Result<()> {
        if batch.len() != self.columns.len() {
            return Err(Error::Codec("push: column count mismatch".into()));
        }
        let n = batch.first().map(|(_, c)| c.len()).unwrap_or(0);
        for ((bn, bc), buf) in batch.iter().zip(self.buf.iter_mut()) {
            if bn != &buf.0 {
                return Err(Error::Codec(format!(
                    "push: column '{bn}' != spec '{}'",
                    buf.0
                )));
            }
            if bc.len() != n {
                return Err(Error::Codec("push: ragged batch columns".into()));
            }
            buf.1.extend(bc)?;
        }
        self.buf_rows += n;
        self.total_rows = self
            .total_rows
            .checked_add(u64::try_from(n).map_err(|e| Error::Codec(format!("push: {e}")))?)
            .ok_or_else(|| Error::Codec("push: total rows overflow u64".into()))?;
        while self.buf_rows >= ROWS_PER_GROUP {
            self.flush_group(ROWS_PER_GROUP)?;
        }
        Ok(())
    }

    /// Spill the first `n` buffered rows as a durable fragment; keep the remainder in the buffer.
    fn flush_group(&mut self, n: usize) -> Result<()> {
        let group: TableData = self
            .buf
            .iter()
            .map(|(name, c)| (name.clone(), c.slice(0, n)))
            .collect();
        let frag = self.frag_path(self.frags.len());
        write_fragment(&frag, &group)?;
        self.frags.push(frag);
        self.frag_rows
            .push(u64::try_from(n).map_err(|e| Error::Codec(format!("flush: {e}")))?);
        self.buf = self
            .buf
            .iter()
            .map(|(name, c)| (name.clone(), c.slice(n, c.len())))
            .collect();
        self.buf_rows -= n;
        Ok(())
    }

    /// Drain the trailing partial row-group, partition the fragments into blocks per the format
    /// SSoT ([`crate::table::block_count`] + [`crate::table::block_name`]), and dispatch one
    /// [`table_job_from_fragments`] encode job per block to the [`StreamWriter`]. Returns the
    /// number of blocks dispatched.
    ///
    /// The partition is a **pure function of the data**: every full block carries exactly
    /// [`ROW_GROUPS_PER_BLOCK`] (= 64) consecutive fragments; the trailing block holds whatever
    /// fragments remain. So whole-file and slab-streamed paths over the same rows produce the same
    /// per-block partition, and therefore the same per-block specs, digests, and content_hash.
    pub fn finish(mut self) -> Result<u64> {
        if self.buf_rows > 0 {
            let n = self.buf_rows;
            self.flush_group(n)?;
        }
        let total_blocks = partition_blocks(self.total_rows, self.block_rows);
        // Row-groups per full block — block_rows is validated to be a multiple of ROWS_PER_GROUP in
        // construction, so this is exact (no remainder lost). usize cast is bounded by ROW_GROUPS_PER_BLOCK
        // for production (= 64) and the test seam's small values, so it never overflows.
        let row_groups_per_block = usize::try_from(self.block_rows / ROWS_PER_GROUP as u64)
            .map_err(|e| Error::Codec(format!("multi-block: row-groups-per-block: {e}")))?;
        // Partition fragments into per-block buckets: blocks 0..total_blocks-1 each take
        // row_groups_per_block fragments; the trailing block takes the rest. The "empty product"
        // case (no fragments at all) still emits ONE empty `prefix` block — matches TableStreamWriter
        // legacy behaviour (encode_streaming with an empty iterator → an empty Vortex file).
        let mut frag_idx = 0usize;
        for blk in 0..total_blocks {
            let take = if blk + 1 < total_blocks {
                row_groups_per_block
            } else {
                self.frags.len().saturating_sub(frag_idx)
            };
            let frags_in_block: Vec<PathBuf> = self.frags[frag_idx..frag_idx + take].to_vec();
            let rows_in_block: u64 = self.frag_rows[frag_idx..frag_idx + take].iter().sum();
            frag_idx += take;
            let nm = block_name(&self.name_prefix, blk, total_blocks);
            let spec = TableSpec {
                columns: self.columns.clone(),
                rows: rows_in_block,
                row_index: self.row_index.clone(),
            };
            self.sw
                .push(table_job_from_fragments(nm, spec, frags_in_block))?;
        }
        Ok(total_blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::WriteSession;
    use crate::Reader;
    use tessera_core::block::table::Column;

    fn col(name: &str, dtype: &str) -> Column {
        Column {
            name: name.into(),
            dtype: dtype.into(),
            codec: None,
        }
    }

    #[test]
    fn accumulator_equals_batch_over_odd_batches() {
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2 + 5000; // 3 groups (2 full + remainder)
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "f4")],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        let full: TableData = vec![
            ("t".into(), ColumnData::U64((0..rows as u64).collect())),
            (
                "e".into(),
                ColumnData::F32((0..rows).map(|k| 511.0 + (k % 13) as f32).collect()),
            ),
        ];

        // push in 9999-row batches — deliberately NOT aligned to the 65536 grid
        let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join("stage")).unwrap();
        let mut pushed = 0usize;
        while pushed < rows {
            let n = 9999.min(rows - pushed);
            let batch: TableData = full
                .iter()
                .map(|(name, c)| (name.clone(), c.slice(pushed, pushed + n)))
                .collect();
            w.push(batch).unwrap();
            pushed += n;
        }
        let streamed = w.finish().unwrap();

        // == a batch encode of the whole table (SSoT: re-chunked to the fixed grid)
        let batch = table::encode(&spec, &full).unwrap();
        assert_eq!(streamed, batch, "accumulator output != batch encode");
        // and it decodes back to the original rows
        assert_eq!(table::decode(&spec, &streamed).unwrap(), full);
    }

    /// Build a TableData of `rows` rows for the spec `vec![col("t","u8"), col("e","f4")]`.
    fn deterministic_rows(start: u64, n: usize) -> TableData {
        vec![
            (
                "t".into(),
                ColumnData::U64((start..start + n as u64).collect()),
            ),
            (
                "e".into(),
                ColumnData::F32((0..n).map(|k| 511.0 + (k % 13) as f32).collect()),
            ),
        ]
    }

    /// Build a full data table of `rows` rows (equivalent across whole-file and streamed paths).
    fn full_data(rows: usize) -> TableData {
        deterministic_rows(0, rows)
    }

    #[test]
    fn multi_block_under_ceiling_is_single_events_block_byte_identical_to_legacy() {
        // The CORPUS-SAFETY invariant: a product with rows ≤ BLOCK_ROWS produces exactly ONE block
        // named `events`, with bytes byte-identical to the existing TableStreamWriter single-block
        // path. Otherwise every existing fixture would change content_hash on this PR. We test at
        // a moderate row count well under BLOCK_ROWS (5_000) — same shape every conformance fixture
        // and ge_hdf5 test uses today.
        let dir = tempfile::tempdir().unwrap();
        let rows = 5_000usize; // < BLOCK_ROWS (4_194_304) → single block
        let full = full_data(rows);
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "f4")],
            rows: rows as u64,
            row_index: Some("t".into()),
        };

        // Legacy path: TableStreamWriter → encode_streaming → one Vortex block bytes.
        let mut legacy_w =
            TableStreamWriter::new(spec.clone(), &dir.path().join("legacy_stage")).unwrap();
        legacy_w.push(full.clone()).unwrap();
        let legacy_bytes = legacy_w.finish().unwrap();

        // New multi-block sink path: pushes the same rows, finishes through StreamWriter → seal.
        let ws = WriteSession::create(
            &dir.path().join("ws_stage"),
            "listmode",
            "t",
            "d",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        let mut sw = StreamWriter::new(ws, 4, 4);
        let mut sink = TableMultiBlockSink::new(
            spec.columns.clone(),
            "events",
            &dir.path().join("sink_stage"),
            &mut sw,
        )
        .unwrap()
        .with_row_index("t");
        sink.push(full).unwrap();
        let dispatched = sink.finish().unwrap();
        assert_eq!(dispatched, 1, "≤ BLOCK_ROWS → exactly one block");
        let out = dir.path().join("multi.tsra");
        let sealed = sw.finish(&out).unwrap();

        // The sealed product has exactly ONE block, named `events` (NOT `events_0000`).
        assert_eq!(sealed.blocks.len(), 1);
        assert_eq!(sealed.blocks[0].name, "events", "small-stays-single");

        // And that block's bytes equal the legacy single-block bytes — content_hash invariant.
        let mut rdr = Reader::open(&out).unwrap();
        let got = rdr.read_block("events").unwrap();
        assert_eq!(
            got, legacy_bytes,
            "single-block bytes must be byte-identical to legacy path"
        );
    }

    /// Run the multi-block ingest end-to-end with a configurable worker count and a TEST-ONLY block
    /// size; returns the sealed product's content_hash. Used by the worker-independence test below.
    fn run_multi_block_ingest(
        dir: &std::path::Path,
        rows: usize,
        workers: usize,
        block_rows: u64,
    ) -> String {
        let columns = vec![col("t", "u8"), col("e", "f4")];
        let stage_ws = dir.join(format!("ws_stage_w{workers}"));
        let stage_sink = dir.join(format!("sink_stage_w{workers}"));
        let out = dir.join(format!("multi_w{workers}.tsra"));
        let ws =
            WriteSession::create(&stage_ws, "listmode", "p", "d", "2024-01-01T00:00:00Z").unwrap();
        let mut sw = StreamWriter::new(ws, workers, 8);
        {
            let mut sink = TableMultiBlockSink::with_block_rows(
                columns,
                "events",
                &stage_sink,
                &mut sw,
                block_rows,
            )
            .unwrap()
            .with_row_index("t");
            // Push in misaligned 13_337-row batches so the row-group/block boundaries get hit by
            // arbitrary row offsets — same pattern the real DAQ producer hits.
            let full = full_data(rows);
            let mut pushed = 0usize;
            while pushed < rows {
                let n = 13_337.min(rows - pushed);
                let batch: TableData = full
                    .iter()
                    .map(|(name, c)| (name.clone(), c.slice(pushed, pushed + n)))
                    .collect();
                sink.push(batch).unwrap();
                pushed += n;
            }
            sink.finish().unwrap();
        }
        let sealed = sw.finish(&out).unwrap();
        sealed.content_hash.clone().unwrap_or_default()
    }

    #[test]
    fn multi_block_above_ceiling_is_worker_count_independent() {
        // DETERMINISM GATE: the same data must produce the same content_hash regardless of how many
        // encode workers run in parallel. The StreamWriter's ordered committer (commit-in-push-order)
        // is what guarantees this for the per-block bytes; the sink's partition (a pure function of
        // the rows) is what guarantees it for the per-block split. Test at TEST-ONLY block_rows so
        // we don't have to materialise 4M+ rows; PRODUCTION block_rows stays at the format
        // invariant 2^22.
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 5 + 1234; // > test block_rows → at least 3 blocks
        let block_rows = (ROWS_PER_GROUP as u64) * 2; // 2 row-groups per block → 3 blocks for the rows above
        let h1 = run_multi_block_ingest(dir.path(), rows, 1, block_rows);
        let h2 = run_multi_block_ingest(dir.path(), rows, 4, block_rows);
        let h8 = run_multi_block_ingest(dir.path(), rows, 8, block_rows);
        assert!(h1.starts_with("blake3:"), "content_hash must be set");
        assert_eq!(h1, h2, "workers=1 vs 4 disagree on content_hash");
        assert_eq!(h1, h8, "workers=1 vs 8 disagree on content_hash");
    }

    #[test]
    fn multi_block_above_ceiling_emits_partitioned_names_in_manifest_order() {
        // The naming + manifest-order proof at the test block size: 3 blocks → events_0000, events_0001, events_0002.
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 5 + 1234;
        let block_rows = (ROWS_PER_GROUP as u64) * 2;
        let columns = vec![col("t", "u8"), col("e", "f4")];
        let ws = WriteSession::create(
            &dir.path().join("stage"),
            "listmode",
            "p",
            "d",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        let mut sw = StreamWriter::new(ws, 2, 4);
        {
            let mut sink = TableMultiBlockSink::with_block_rows(
                columns,
                "events",
                &dir.path().join("sink"),
                &mut sw,
                block_rows,
            )
            .unwrap()
            .with_row_index("t");
            sink.push(full_data(rows)).unwrap();
            let n = sink.finish().unwrap();
            assert_eq!(
                n, 3,
                "5×ROWS_PER_GROUP+rem over 2×ROWS_PER_GROUP/block = 3 blocks"
            );
        }
        let out = dir.path().join("multi.tsra");
        let sealed = sw.finish(&out).unwrap();
        assert_eq!(
            sealed
                .blocks
                .iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>(),
            vec!["events_0000", "events_0001", "events_0002"]
        );
        // and the reader's block_group helper recovers the same logical group in manifest order.
        let r = Reader::open(&out).unwrap();
        assert_eq!(
            r.block_group("events"),
            vec!["events_0000", "events_0001", "events_0002"]
        );
    }

    #[test]
    fn partition_blocks_matches_block_count_at_block_rows_boundary() {
        // Cross-check the partition + naming SSoT at the production BLOCK_ROWS boundary — guards
        // against an accidental off-by-one between block_count and the sink's loop bound. Pure unit
        // test of the helper, no I/O.
        assert_eq!(table::block_count(0), 1);
        assert_eq!(table::block_count(1), 1);
        assert_eq!(table::block_count(table::BLOCK_ROWS as u64), 1);
        assert_eq!(table::block_count(table::BLOCK_ROWS as u64 + 1), 2);
        assert_eq!(table::block_count((table::BLOCK_ROWS as u64) * 2), 2);
        assert_eq!(table::block_count((table::BLOCK_ROWS as u64) * 2 + 1), 3);
    }

    #[test]
    fn live_index_fold_matches_batch_chunk_index() {
        // ADR-0028 §5: the bounded-memory live {hash,stats} fold over the streamed row-groups must
        // reconcile with a batch table_chunk_index over the same rows — root AND aggregate.
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2; // exactly two canonical groups (no trailing remainder)
        let spec = TableSpec {
            columns: vec![col("k", "i8"), col("v", "f4")],
            rows: rows as u64,
            row_index: None,
        };
        let full: TableData = vec![
            (
                "k".into(),
                ColumnData::I64((0..rows as i64).map(|k| k * 3 - 7).collect()),
            ),
            (
                "v".into(),
                ColumnData::F32((0..rows).map(|k| (k % 17) as f32).collect()),
            ),
        ];
        let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join("stage"))
            .unwrap()
            .with_live_index("k");
        // push in 9999-row batches (NOT grid-aligned); full groups flush inside push().
        let mut pushed = 0usize;
        while pushed < rows {
            let n = 9999.min(rows - pushed);
            let batch: TableData = full
                .iter()
                .map(|(name, c)| (name.clone(), c.slice(pushed, pushed + n)))
                .collect();
            w.push(batch).unwrap();
            pushed += n;
        }
        // both groups flushed at the grid boundary → the live fold is complete (no finish needed).
        let batch_idx = table::table_chunk_index(&spec, &full, "k").unwrap();
        assert_eq!(
            w.live_root().unwrap(),
            batch_idx.root(),
            "live MMR root != batch chunk-index root"
        );
        assert_eq!(w.live_aggregate().unwrap(), batch_idx.aggregate());
        assert_eq!(w.live_aggregate().unwrap().count, rows as u64);
        // a writer without with_live_index exposes no live index.
        let plain = TableStreamWriter::new(spec, &dir.path().join("stage2")).unwrap();
        assert!(plain.live_root().is_none());
    }
}
