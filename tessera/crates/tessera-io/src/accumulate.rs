//! Row-group accumulator (ROADMAP P3 / #203, ADR-0026) — the DAQ-facing producer for **table**
//! blocks.
//!
//! A device streams arbitrary-size transport batches; `TableStreamWriter` re-chunks them to Tessera's
//! fixed [`ROWS_PER_GROUP`] grid, spills each full row-group to a **durable fragment** (so RAM stays
//! bounded at ~2 row-groups no matter how many rows are pushed), and at `finish` **lazily** compacts
//! the fragments through [`table::encode_streaming_to`] into the **one canonical chunked table
//! block** — byte-identical to a batch encode of the same rows. The encoded block goes to any
//! [`std::io::Write`] ([`TableStreamWriter::finish_to`]) or, as a staged job, into a
//! [`StreamWriter`] ([`TableStreamWriter::into_job`]), so the encoded block is never collected in RAM
//! (#677; the Vortex writer's own in-flight segments are a known limit, #681).
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
use tessera_core::{Error, Result};

use crate::stream::{table_job_from_fragments, StagedEncodeJob, StreamWriter};
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
                ColumnData::empty_for(c).expect("validated dtype"),
            )
        })
        .collect()
}

/// Magic + version prefixing every staged fragment. A fragment is transient staging state, but it
/// is `sync_all`'d for crash recovery, so a stage dir written by an older binary can outlive it.
/// The v1 format began directly with `u64 n_rows` and derived each column's length from
/// `dtype_size`; parsing a v1 fragment with the v2 reader (or vice versa) would silently split
/// the columns at the wrong offsets rather than fail, so the version is explicit.
const FRAG_MAGIC: &[u8; 8] = b"TSRAFRG\x02";

/// Write one row-group [`TableData`] to a durable fragment file — the internal staging format the
/// streaming table writers spill to (NOT the sealed format). Shape:
/// `magic · u64 le n_rows · per column { u64 le byte_len · le_bytes }`, columns in spec order.
///
/// The per-column byte length is what makes **variable-width** columns representable: `str` has no
/// element size, so a reader cannot derive its extent from the row count. Storing the length also
/// removes the reader's dependence on `dtype_size` entirely, which is what previously restricted
/// staging to fixed-width numerics.
///
/// Shared by [`TableStreamWriter`] (single block) and the multi-block sink so both stage in the
/// **same** format, and so the parallel per-block encode jobs read fragments written by either path.
pub(crate) fn write_fragment(path: &Path, group: &TableData) -> Result<()> {
    let n_rows = u64::try_from(group.first().map(|(_, c)| c.len()).unwrap_or(0))
        .map_err(|e| Error::Codec(format!("fragment: row count overflows u64: {e}")))?;
    let mut f = File::create(path)?;
    f.write_all(FRAG_MAGIC)?;
    f.write_all(&n_rows.to_le_bytes())?;
    for (_, c) in group {
        let raw = c.to_le_bytes();
        let len = u64::try_from(raw.len())
            .map_err(|e| Error::Codec(format!("fragment: column length overflows u64: {e}")))?;
        f.write_all(&len.to_le_bytes())?;
        f.write_all(&raw)?;
    }
    f.sync_all()?; // durable: a committed row-group survives a crash
    Ok(())
}

/// Read one fragment back into a [`TableData`] of `columns` (inverse of [`write_fragment`]). The
/// fragment format is internal; this is the only reader. `pub(crate)` so [`crate::stream`] can lazily
/// pull fragments inside a per-block encode job.
pub(crate) fn read_fragment(path: &Path, columns: &[Column]) -> Result<TableData> {
    let bytes = fs::read(path)?;
    let magic = bytes
        .get(0..8)
        .ok_or_else(|| Error::Codec("fragment: truncated header".into()))?;
    if magic != FRAG_MAGIC {
        return Err(Error::Codec(format!(
            "fragment: bad magic (stale or foreign staging file at {})",
            path.display()
        )));
    }
    let head_arr: [u8; 8] = bytes
        .get(8..16)
        .ok_or_else(|| Error::Codec("fragment: truncated header".into()))?
        .try_into()
        .map_err(|e| Error::Codec(format!("fragment: corrupt header: {e}")))?;
    let n_rows = usize::try_from(u64::from_le_bytes(head_arr))
        .map_err(|e| Error::Codec(format!("fragment: row count exceeds usize: {e}")))?;
    let mut off = 16usize;
    let mut out = Vec::with_capacity(columns.len());
    for c in columns {
        let len_arr: [u8; 8] = bytes
            .get(off..off + 8)
            .ok_or_else(|| {
                Error::Codec(format!(
                    "fragment: truncated length for column '{}'",
                    c.name
                ))
            })?
            .try_into()
            .map_err(|e| Error::Codec(format!("fragment: corrupt column length: {e}")))?;
        let len = usize::try_from(u64::from_le_bytes(len_arr))
            .map_err(|e| Error::Codec(format!("fragment: column length exceeds usize: {e}")))?;
        off += 8;
        let raw = bytes
            .get(off..off + len)
            .ok_or_else(|| Error::Codec(format!("fragment: truncated column '{}'", c.name)))?;
        let col = ColumnData::from_column_bytes(c, raw, n_rows)?;
        if col.len() != n_rows {
            return Err(Error::Codec(format!(
                "fragment: column '{}' decoded {} rows, header says {n_rows}",
                c.name,
                col.len()
            )));
        }
        out.push((c.name.clone(), col));
        off += len;
    }
    Ok(out)
}

/// The first read error of a [`fragment_groups`] iterator, checked after the encode consumed it.
struct FragmentReadError(Arc<Mutex<Option<Error>>>);

impl FragmentReadError {
    /// `Err` with the first fragment read error, if any. The encode that consumed the iterator then
    /// saw empty row-groups in place of the failed ones, so its output must be discarded.
    fn check(self) -> Result<()> {
        match self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// The row-groups staged in `paths`, read lazily one fragment at a time — the input of
/// [`compact_fragments_to`]. The encoder pulls infallible [`TableData`], so a read error is stashed
/// in the returned [`FragmentReadError`] and empty row-groups are fed from then on; the caller must
/// [`FragmentReadError::check`] it after the encode.
fn fragment_groups(
    columns: Vec<Column>,
    paths: Vec<PathBuf>,
) -> (
    impl Iterator<Item = TableData> + Send + 'static,
    FragmentReadError,
) {
    let err: Arc<Mutex<Option<Error>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&err);
    let groups = paths.into_iter().map(move |p| {
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return empty_cols(&columns);
        }
        match read_fragment(&p, &columns) {
            Ok(td) => td,
            Err(e) => {
                *slot = Some(e);
                empty_cols(&columns)
            }
        }
    });
    (groups, FragmentReadError(err))
}

/// Encode the row-groups staged in `paths` as one table block of `spec` into `sink` — every fragment
/// compaction ([`TableStreamWriter::finish_to`] and [`table_job_from_fragments`]). Returns the bytes
/// written.
///
/// **A fragment read error wins over the encode's error.** After a failed read the encoder is fed
/// empty row-groups, so the read error is the first cause, and whatever the encode then reports
/// (its own failure, or a short block that is discarded) is downstream of it or beside the point.
pub(crate) fn compact_fragments_to<W: Write>(
    spec: &TableSpec,
    paths: Vec<PathBuf>,
    sink: W,
) -> Result<u64> {
    let (groups, read_error) = fragment_groups(spec.columns.clone(), paths);
    let encoded = table::encode_streaming_to(spec, groups, sink);
    read_error.check()?;
    encoded
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
        // `validate_dtype`, NOT `dtype_size`: the latter rejects the variable-width `str` (and,
        // before it was added there, `b1`), which made this the front door that locked boolean and
        // string columns out of every streaming/DAQ/ingest write path.
        for c in &spec.columns {
            ColumnData::validate_dtype(&c.dtype)?;
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
        let mut group: TableData = self
            .buf
            .iter()
            .map(|(name, c)| (name.clone(), c.slice(0, n)))
            .collect();
        write_fragment(&self.frag_path(self.n_frags), &group)?;
        self.n_frags += 1;
        // ADR-0028 §5: fold this group's {digest, stats} into the live MMR (bounded memory — the group
        // is folded then dropped). The per-group digest is `table_chunk_index`'s own
        // (`row_group_digest`), so the live root reconciles with the batch index. The fragment is
        // already written with the producer's raw bytes, so the digest may canonicalise `group` in
        // place.
        if let Some(col) = &self.stat_column {
            let stats = group
                .iter()
                .find(|(n, _)| n == col)
                .and_then(|(_, c)| c.chunk_stats())
                .unwrap_or_else(ChunkStats::identity);
            self.fold.push(
                &table::row_group_digest(group.iter_mut().map(|(_, c)| c)),
                stats,
            );
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
    /// chunked table block (byte-identical to a batch encode of every pushed row, in order),
    /// returned in RAM. The whole encoded block is one `Vec`, so this is for tables that fit in
    /// memory; [`Self::finish_to`] and [`Self::into_job`] keep it out of RAM.
    pub fn finish(self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.finish_to(&mut bytes)?;
        Ok(bytes)
    }

    /// [`Self::finish`], writing the block to `sink` as it is encoded instead of returning it:
    /// the encode holds one row-group of input and doesn't collect the output (see
    /// [`table::encode_streaming_to`]). Returns the bytes written. The bytes are those of
    /// [`Self::finish`]. On error, what `sink` received is not a valid block.
    pub fn finish_to<W: Write>(mut self, sink: W) -> Result<u64> {
        let paths = self.flush_trailing()?;
        compact_fragments_to(&self.spec, paths, sink)
    }

    /// Flush the trailing partial group and hand the compaction to a [`StreamWriter`] as a
    /// [`StagedEncodeJob`] for the block `name` ([`table_job_from_fragments`] over this writer's
    /// fragments): it encodes on a worker into the session's staging directory and commits from
    /// there, so the block's size doesn't count against the window (#677). Push it with
    /// [`StreamWriter::push_staged`]. The fragments stay in this writer's stage directory until the
    /// job has read them; removing them is the caller's, as with [`Self::finish`].
    pub fn into_job(mut self, name: impl Into<String>) -> Result<StagedEncodeJob> {
        let paths = self.flush_trailing()?;
        Ok(table_job_from_fragments(name, self.spec.clone(), paths))
    }

    /// Spill the trailing partial row-group, then return every fragment's path in append order.
    fn flush_trailing(&mut self) -> Result<Vec<PathBuf>> {
        if self.buf_rows > 0 {
            self.flush(self.buf_rows)?;
        }
        Ok((0..self.n_frags).map(|i| self.frag_path(i)).collect())
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
            ColumnData::validate_dtype(&c.dtype)?;
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
                .push_staged(table_job_from_fragments(nm, spec, frags_in_block))?;
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
            ..Default::default()
        }
    }

    /// Streaming a table containing a `b1` and a `str` column must work and must be byte-identical
    /// to a batch encode of the same rows.
    ///
    /// Regression for the half-landed #354: `TableStreamWriter::new` validated dtypes with
    /// `ColumnData::dtype_size`, which errors for the variable-width `str` (and errored for `b1`
    /// too), so constructing a writer over the new column types failed outright — every
    /// streaming/DAQ/ingest path was locked out of the very types #354 added. The fragment reader
    /// then compounded it by deriving each column's extent as `dtype_size * n_rows`, which is
    /// structurally wrong for length-prefixed `Utf8` regardless of validation.
    #[test]
    fn streaming_bool_and_utf8_equals_batch_encode() {
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP + 4321; // spans a group boundary + a remainder
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("flag", "b1"), col("origin", "str")],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        // Deliberately variable-length strings, so a fixed-width offset assumption cannot survive.
        let origins: Vec<String> = (0..rows)
            .map(|k| match k % 4 {
                0 => "annih511".to_string(),
                1 => "prompt_nuclear".to_string(),
                2 => String::new(),
                _ => format!("scatter-{k}"),
            })
            .collect();
        let full: TableData = vec![
            ("t".into(), ColumnData::U64((0..rows as u64).collect())),
            (
                "flag".into(),
                ColumnData::Bool((0..rows).map(|k| k % 3 == 0).collect()),
            ),
            ("origin".into(), ColumnData::Utf8(origins)),
        ];

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

        let batch = table::encode(&spec, &full).unwrap();
        assert_eq!(
            streamed, batch,
            "streamed b1/str block != batch encode of the same rows"
        );
        assert_eq!(
            table::decode(&spec, &streamed).unwrap(),
            full,
            "b1/str roundtrip through the streaming path lost data"
        );
    }

    /// A fragment round-trips a variable-width column exactly. Targets the fragment format itself
    /// rather than the whole writer, so a regression in the length framing is attributed directly.
    #[test]
    fn fragment_roundtrips_variable_width_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.frag");
        let columns = vec![col("flag", "b1"), col("origin", "str"), col("t", "u8")];
        let group: TableData = vec![
            ("flag".into(), ColumnData::Bool(vec![true, false, true])),
            (
                "origin".into(),
                ColumnData::Utf8(vec!["".into(), "a-much-longer-value".into(), "x".into()]),
            ),
            ("t".into(), ColumnData::U64(vec![7, 8, 9])),
        ];
        write_fragment(&path, &group).unwrap();
        assert_eq!(read_fragment(&path, &columns).unwrap(), group);
    }

    /// A staging file that is not a v2 fragment fails loudly instead of splitting columns wrongly.
    #[test]
    fn fragment_with_bad_magic_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.frag");
        // v1 shape: bare `u64 n_rows` then raw column bytes, no magic.
        let mut bytes = 3u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 0, 1]);
        std::fs::write(&path, &bytes).unwrap();
        let err = read_fragment(&path, &[col("flag", "b1")]).unwrap_err();
        assert!(
            format!("{err}").contains("bad magic"),
            "expected a magic mismatch, got: {err}"
        );
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

    /// #677: the out-of-RAM compactions emit `finish`'s bytes — [`TableStreamWriter::finish_to`] into
    /// a file, and [`TableStreamWriter::into_job`] as a staged block through a `StreamWriter` —
    /// over a nullable column, and a fragment lost before the compaction is that error, not a
    /// silently short block.
    #[test]
    fn finish_to_and_into_job_emit_the_bytes_of_finish() {
        use crate::stream::{StreamWindow, StreamWriter};
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2 + 5000;
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("k", "i8").nullable()],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        let full: TableData = vec![
            ("t".into(), ColumnData::U64((0..rows as u64).collect())),
            (
                "k".into(),
                ColumnData::Nullable {
                    values: Box::new(ColumnData::I64((0..rows as i64).map(|k| k * 7).collect())),
                    validity: (0..rows).map(|r| r % 3 != 0).collect(),
                },
            ),
        ];
        let writer = |tag: &str| {
            let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join(tag)).unwrap();
            for start in (0..rows).step_by(9999) {
                let end = (start + 9999).min(rows);
                w.push(
                    full.iter()
                        .map(|(n, c)| (n.clone(), c.slice(start, end)))
                        .collect(),
                )
                .unwrap();
            }
            w
        };
        let batch = table::encode(&spec, &full).unwrap();
        assert_eq!(writer("vec").finish().unwrap(), batch);

        let file = dir.path().join("block.vortex");
        let written = writer("file")
            .finish_to(std::io::BufWriter::new(File::create(&file).unwrap()))
            .unwrap();
        assert_eq!(written, batch.len() as u64);
        assert!(
            fs::read(&file).unwrap() == batch,
            "finish_to bytes != finish"
        );

        let stage = dir.path().join("session");
        let ws =
            crate::WriteSession::create(&stage, "recon", "p", "d", "2024-01-01T00:00:00Z").unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(2, 1));
        sw.push_staged(writer("job").into_job("events").unwrap())
            .unwrap();
        let out = dir.path().join("p.tsra");
        sw.finish(&out).unwrap();
        let mut rdr = crate::Reader::open(&out).unwrap();
        assert!(
            rdr.read_block("events").unwrap() == batch,
            "into_job block != finish"
        );

        // A fragment removed before the compaction reads it: the read error, nothing written.
        let w = writer("lost");
        fs::remove_file(dir.path().join("lost").join("g00000001.frag")).unwrap();
        let err = w.finish_to(std::io::sink()).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("no such file"),
            "{err}"
        );
    }

    /// #677 r2 item 4: when a fragment can't be read AND the encode fails, the caller sees the
    /// fragment read error, the first cause — not the encode's error, which hid it. Both
    /// compactions ([`TableStreamWriter::finish_to`] and [`table_job_from_fragments`]) run
    /// `compact_fragments_to`, so this covers both.
    #[test]
    fn a_fragment_read_error_is_what_the_caller_sees_when_the_encode_fails_too() {
        /// A sink that fills up after its first 8 bytes (the file's leading magic), as a disk does.
        struct Full(usize);
        impl Write for Full {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.0 >= 8 {
                    return Err(std::io::Error::other("sink is full"));
                }
                let n = buf.len().min(8 - self.0);
                self.0 += n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2 + 77;
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "f4")],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        let lost = |tag: &str| {
            let stage = dir.path().join(tag);
            let mut w = TableStreamWriter::new(spec.clone(), &stage).unwrap();
            w.push(deterministic_rows(0, rows)).unwrap();
            w.flush_trailing().unwrap();
            fs::remove_file(stage.join("g00000000.frag")).unwrap();
            w
        };
        let is_read_error = |err: &Error| err.to_string().to_lowercase().contains("no such file");

        // The full sink alone fails the encode: that is the error that masked the read error.
        let w = TableStreamWriter::new(spec.clone(), &dir.path().join("full")).unwrap();
        let err = w.finish_to(Full(0)).unwrap_err();
        assert!(err.to_string().contains("sink is full"), "{err}");

        let err = lost("lost").finish_to(Full(0)).unwrap_err();
        assert!(
            is_read_error(&err),
            "the encode error masked the read error: {err}"
        );
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

    /// Push `full` into `w` in 9999-row batches — NOT grid-aligned, so row-groups are cut inside
    /// `push`, across batch boundaries.
    fn push_in_batches(w: &mut TableStreamWriter, full: &TableData) {
        let rows = full.first().map(|(_, c)| c.len()).unwrap_or(0);
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
    }

    /// #627: the live fold reconciles with the batch `table_chunk_index` — root AND aggregate —
    /// over a NULLABLE stat column whose NULLs sit in only one of the two row-groups, next to a
    /// nullable float column holding NaN and NULL. The batch side used to panic on these rows.
    #[test]
    fn live_index_fold_matches_batch_chunk_index_over_nullable_columns() {
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2;
        let spec = TableSpec {
            columns: vec![col("k", "i8").nullable(), col("v", "f4").nullable()],
            rows: rows as u64,
            row_index: None,
        };
        // NULLs in the first row-group only; i64::MAX under each so a leak would show as the max.
        let k_valid: Vec<bool> = (0..rows)
            .map(|r| r >= ROWS_PER_GROUP || r % 5 != 0)
            .collect();
        let full: TableData = vec![
            (
                "k".into(),
                ColumnData::Nullable {
                    values: Box::new(ColumnData::I64(
                        (0..rows)
                            .map(|r| {
                                if k_valid[r] {
                                    r as i64 * 3 - 7
                                } else {
                                    i64::MAX
                                }
                            })
                            .collect(),
                    )),
                    validity: k_valid.clone(),
                },
            ),
            (
                "v".into(),
                ColumnData::Nullable {
                    values: Box::new(ColumnData::F32(
                        (0..rows)
                            .map(|r| {
                                if r % 11 == 0 {
                                    f32::NAN
                                } else {
                                    (r % 17) as f32
                                }
                            })
                            .collect(),
                    )),
                    validity: (0..rows).map(|r| r % 7 != 0).collect(),
                },
            ),
        ];
        let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join("stage"))
            .unwrap()
            .with_live_index("k");
        push_in_batches(&mut w, &full);
        let batch_idx = table::table_chunk_index(&spec, &full, "k").unwrap();
        assert_eq!(w.live_root().unwrap(), batch_idx.root());
        let agg = w.live_aggregate().unwrap();
        assert_eq!(agg, batch_idx.aggregate());
        assert_eq!(agg.count, k_valid.iter().filter(|&&ok| ok).count() as u64);
        assert_eq!(agg.max, Some((rows as i64 - 1) * 3 - 7));
    }

    /// #627: an all-NULL row-group, and a `u64::MAX` under a NULL, reconcile between the live fold
    /// and the batch index. The all-NULL group folds the empty chunk's stats; the value under a
    /// NULL is invisible to both paths, so neither errors nor drops the group.
    #[test]
    fn live_index_fold_matches_batch_over_an_all_null_group_and_u64_under_null() {
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2;
        let spec = TableSpec {
            columns: vec![col("k", "u8").nullable()],
            rows: rows as u64,
            row_index: None,
        };
        // group 0: every row NULL; group 1: every 4th row NULL. u64::MAX under every NULL.
        let validity: Vec<bool> = (0..rows)
            .map(|r| r >= ROWS_PER_GROUP && r % 4 != 0)
            .collect();
        let values: Vec<u64> = (0..rows)
            .map(|r| if validity[r] { r as u64 } else { u64::MAX })
            .collect();
        let full: TableData = vec![(
            "k".into(),
            ColumnData::Nullable {
                values: Box::new(ColumnData::U64(values)),
                validity: validity.clone(),
            },
        )];
        let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join("stage"))
            .unwrap()
            .with_live_index("k");
        push_in_batches(&mut w, &full);
        let batch_idx = table::table_chunk_index(&spec, &full, "k").unwrap();
        assert_eq!(batch_idx.stat_pyramid()[0][0], ChunkStats::identity());
        assert_eq!(w.live_root().unwrap(), batch_idx.root());
        let agg = w.live_aggregate().unwrap();
        assert_eq!(agg, batch_idx.aggregate());
        assert_eq!(agg.count, validity.iter().filter(|&&ok| ok).count() as u64);
        assert_eq!(agg.max, Some(rows as i64 - 1));
    }

    /// #632: the streaming live fold honours the same invariant as `encode` — two producers that
    /// differ ONLY beneath their NULLs stream to identical block bytes AND an identical live
    /// chunk-index root, across a trailing partial row-group, and that root is the batch
    /// `table_chunk_index` root of the canonical table.
    #[test]
    fn live_index_fold_is_independent_of_values_under_nulls() {
        use crate::table::under_null_fixture as fx;
        let dir = tempfile::tempdir().unwrap();
        let rows = ROWS_PER_GROUP * 2 + 777; // two full groups + a partial one flushed by finish
        let spec = fx::spec(rows);
        let (canonical, dirty) = fx::tables(rows);
        let stream = |data: &TableData, stage: &str| {
            let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join(stage))
                .unwrap()
                .with_live_index("k");
            push_in_batches(&mut w, data);
            // `finish` consumes the writer, so read the trailing group's fold through a flush first.
            w.flush(w.buf_rows).unwrap();
            let root = w.live_root().unwrap();
            let aggregate = w.live_aggregate().unwrap();
            (root, aggregate, w.finish().unwrap())
        };
        let (want_root, want_agg, want_bytes) = stream(&canonical, "canonical");
        let (got_root, got_agg, got_bytes) = stream(&dirty, "dirty");
        assert_eq!(got_bytes, want_bytes, "streamed block bytes");
        assert_eq!(got_root, want_root, "live chunk-index root");
        assert_eq!(got_agg, want_agg, "live stats");
        let batch = table::table_chunk_index(&spec, &canonical, "k").unwrap();
        assert_eq!(batch.len(), 3, "precondition: a multi-row-group index");
        assert_eq!(want_root, batch.root(), "batch == streaming");
    }
}
