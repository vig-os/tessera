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
use tessera_core::{Error, Result};

use crate::table::{self, ColumnData, TableData, ROWS_PER_GROUP};

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

fn write_fragment(path: &Path, group: &TableData) -> Result<()> {
    let n_rows = group.first().map(|(_, c)| c.len()).unwrap_or(0) as u64;
    let mut f = File::create(path)?;
    f.write_all(&n_rows.to_le_bytes())?;
    for (_, c) in group {
        f.write_all(&c.to_le_bytes())?;
    }
    f.sync_all()?; // durable: a committed row-group survives a crash
    Ok(())
}

fn read_fragment(path: &Path, columns: &[Column]) -> Result<TableData> {
    let bytes = fs::read(path)?;
    let head = bytes
        .get(0..8)
        .ok_or_else(|| Error::Codec("fragment: truncated header".into()))?;
    let n_rows = u64::from_le_bytes(head.try_into().unwrap()) as usize;
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
        })
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
