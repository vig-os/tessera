//! Table block backend — columnar storage as a deterministic **Vortex** file (the settled table
//! backend: smallest + O(1) random-take + filter-pushdown + zero-copy Arrow→DuckDB, spike
//! S0/S4/S7/S10/S11). The real codec behind a [`TableSpec`] block: a set of typed columns ⇄ the
//! exact bytes stored at `blocks/<name>` in a `.tsra`, and back.
//!
//! Like [`crate::array`], the payload is one self-contained, **byte-deterministic** blob (verified:
//! the Vortex 0.75 file writer produces identical bytes for identical input — the writer-determinism
//! release gate), digested over the encoded bytes. Column dtypes use the fd5 numpy-style codes
//! (`i1/i2/i4/i8`, `u1/u2/u4/u8`, `f4/f8`) carried in [`tessera_core::block::table::Column`].

use futures::StreamExt;
use tessera_core::block::table::TableSpec;
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::chunk_index::{ChunkIndex, ChunkStats};
use tessera_core::hash::digest;
use tessera_core::{Error, Result};
use vortex_array::arrays::struct_::StructArrayExt;
use vortex_array::arrays::{ChunkedArray, PrimitiveArray, StructArray};
use vortex_array::expr::{root, select};
use vortex_array::iter::{ArrayIteratorAdapter, ArrayIteratorExt};
use vortex_array::scalar_fn::session::ScalarFnSession;
use vortex_array::session::ArraySession;
use vortex_array::{ArrayRef, IntoArray, VortexSessionExecute};
use vortex_btrblocks::schemes::float::{ALPRDScheme, ALPScheme};
use vortex_btrblocks::{BtrBlocksCompressorBuilder, SchemeExt};
use vortex_buffer::{Buffer, ByteBuffer, ByteBufferMut};
use vortex_file::{
    register_default_encodings, OpenOptionsSessionExt, WriteOptionsSessionExt, WriteStrategyBuilder,
};
use vortex_io::runtime::current::CurrentThreadRuntime;
use vortex_io::runtime::BlockingRuntime;
use vortex_io::session::{RuntimeSession, RuntimeSessionExt};
use vortex_layout::session::LayoutSession;
use vortex_session::VortexSession;

use crate::BlockPayload;

/// One column's typed values (C order). Covers the numeric dtypes Vortex stores natively; the fd5
/// numpy code (`i2`, `u4`, `f4`, …) names the dtype in the [`TableSpec`].
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl ColumnData {
    /// The fd5 numpy-style dtype code (matches [`tessera_core::block::table::Column::dtype`]).
    pub fn numpy_code(&self) -> &'static str {
        match self {
            ColumnData::I8(_) => "i1",
            ColumnData::I16(_) => "i2",
            ColumnData::I32(_) => "i4",
            ColumnData::I64(_) => "i8",
            ColumnData::U8(_) => "u1",
            ColumnData::U16(_) => "u2",
            ColumnData::U32(_) => "u4",
            ColumnData::U64(_) => "u8",
            ColumnData::F32(_) => "f4",
            ColumnData::F64(_) => "f8",
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ColumnData::I8(v) => v.len(),
            ColumnData::I16(v) => v.len(),
            ColumnData::I32(v) => v.len(),
            ColumnData::I64(v) => v.len(),
            ColumnData::U8(v) => v.len(),
            ColumnData::U16(v) => v.len(),
            ColumnData::U32(v) => v.len(),
            ColumnData::U64(v) => v.len(),
            ColumnData::F32(v) => v.len(),
            ColumnData::F64(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The column's values as `i64` for chunk-statistics (ADR-0028 §3), if it is an integer column that
    /// fits losslessly: `i1/i2/i4/i8`, `u1/u2/u4` always, and `u8` (u64) only when every value ≤
    /// `i64::MAX` (a monotonic cast → `min`/`max` stay exact). Float columns return `None` (they need
    /// canonical reduction before stats — ADR-0024).
    pub fn as_i64(&self) -> Option<Vec<i64>> {
        match self {
            ColumnData::I8(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ColumnData::I16(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ColumnData::I32(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ColumnData::I64(v) => Some(v.clone()),
            ColumnData::U8(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ColumnData::U16(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ColumnData::U32(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ColumnData::U64(v) => v
                .iter()
                .all(|&x| x <= i64::MAX as u64)
                .then(|| v.iter().map(|&x| x as i64).collect()),
            ColumnData::F32(_) | ColumnData::F64(_) => None,
        }
    }

    /// Flatten the column to little-endian bytes for zero-copy reconstruction in another runtime —
    /// e.g. `numpy.frombuffer(buf, "<" + numpy_code)`.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        use crate::array::le_bytes;
        match self {
            ColumnData::I8(v) => v.iter().map(|x| *x as u8).collect(),
            ColumnData::I16(v) => le_bytes(v, i16::to_le_bytes),
            ColumnData::I32(v) => le_bytes(v, i32::to_le_bytes),
            ColumnData::I64(v) => le_bytes(v, i64::to_le_bytes),
            ColumnData::U8(v) => v.clone(),
            ColumnData::U16(v) => le_bytes(v, u16::to_le_bytes),
            ColumnData::U32(v) => le_bytes(v, u32::to_le_bytes),
            ColumnData::U64(v) => le_bytes(v, u64::to_le_bytes),
            ColumnData::F32(v) => le_bytes(v, f32::to_le_bytes),
            ColumnData::F64(v) => le_bytes(v, f64::to_le_bytes),
        }
    }

    /// Build a [`ColumnData`] from a little-endian buffer + numpy code (`i1/i2/i4/i8`, `u1/u2/u4/u8`,
    /// `f4/f8`) — the inverse of [`Self::to_le_bytes`] + [`Self::numpy_code`].
    pub fn from_le_bytes(numpy_code: &str, bytes: &[u8]) -> Result<ColumnData> {
        use crate::array::from_le;
        Ok(match numpy_code {
            "i1" => ColumnData::I8(bytes.iter().map(|&b| b as i8).collect()),
            "i2" => ColumnData::I16(from_le(bytes, i16::from_le_bytes)?),
            "i4" => ColumnData::I32(from_le(bytes, i32::from_le_bytes)?),
            "i8" => ColumnData::I64(from_le(bytes, i64::from_le_bytes)?),
            "u1" => ColumnData::U8(bytes.to_vec()),
            "u2" => ColumnData::U16(from_le(bytes, u16::from_le_bytes)?),
            "u4" => ColumnData::U32(from_le(bytes, u32::from_le_bytes)?),
            "u8" => ColumnData::U64(from_le(bytes, u64::from_le_bytes)?),
            "f4" => ColumnData::F32(from_le(bytes, f32::from_le_bytes)?),
            "f8" => ColumnData::F64(from_le(bytes, f64::from_le_bytes)?),
            other => {
                return Err(Error::Codec(format!(
                    "unsupported column dtype code '{other}'"
                )))
            }
        })
    }

    /// The `[start, end)` row sub-range of this column — used to slice a table into row-groups.
    pub fn slice(&self, start: usize, end: usize) -> ColumnData {
        macro_rules! sl {
            ($v:expr, $variant:ident) => {
                ColumnData::$variant($v[start..end].to_vec())
            };
        }
        match self {
            ColumnData::I8(v) => sl!(v, I8),
            ColumnData::I16(v) => sl!(v, I16),
            ColumnData::I32(v) => sl!(v, I32),
            ColumnData::I64(v) => sl!(v, I64),
            ColumnData::U8(v) => sl!(v, U8),
            ColumnData::U16(v) => sl!(v, U16),
            ColumnData::U32(v) => sl!(v, U32),
            ColumnData::U64(v) => sl!(v, U64),
            ColumnData::F32(v) => sl!(v, F32),
            ColumnData::F64(v) => sl!(v, F64),
        }
    }

    /// Append another column's values onto this one (dtypes must match).
    pub fn extend(&mut self, other: &ColumnData) -> Result<()> {
        macro_rules! ext {
            ($v:expr, $variant:ident) => {
                match other {
                    ColumnData::$variant(o) => $v.extend_from_slice(o),
                    _ => {
                        return Err(Error::Codec(format!(
                            "extend: dtype {} != {}",
                            other.numpy_code(),
                            self.numpy_code()
                        )))
                    }
                }
            };
        }
        match self {
            ColumnData::I8(v) => ext!(v, I8),
            ColumnData::I16(v) => ext!(v, I16),
            ColumnData::I32(v) => ext!(v, I32),
            ColumnData::I64(v) => ext!(v, I64),
            ColumnData::U8(v) => ext!(v, U8),
            ColumnData::U16(v) => ext!(v, U16),
            ColumnData::U32(v) => ext!(v, U32),
            ColumnData::U64(v) => ext!(v, U64),
            ColumnData::F32(v) => ext!(v, F32),
            ColumnData::F64(v) => ext!(v, F64),
        }
        Ok(())
    }

    /// Bytes per element of the numpy dtype code (`i1`=1 … `f8`=8).
    pub fn dtype_size(code: &str) -> Result<usize> {
        Ok(match code {
            "i1" | "u1" => 1,
            "i2" | "u2" => 2,
            "i4" | "u4" | "f4" => 4,
            "i8" | "u8" | "f8" => 8,
            other => return Err(Error::Codec(format!("unknown dtype code '{other}'"))),
        })
    }

    fn to_vortex(&self) -> ArrayRef {
        match self {
            ColumnData::I8(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::I16(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::I32(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::I64(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::U8(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::U16(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::U32(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::U64(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::F32(v) => Buffer::copy_from(v.as_slice()).into_array(),
            ColumnData::F64(v) => Buffer::copy_from(v.as_slice()).into_array(),
        }
    }
}

/// An ordered set of named columns — the decoded form of a table block (column order matches the
/// [`TableSpec`]). All columns have the same length (`rows`).
pub type TableData = Vec<(String, ColumnData)>;

/// An empty column of the dtype named by a numpy code (the decode accumulator).
fn empty_column(code: &str) -> Result<ColumnData> {
    Ok(match code {
        "i1" => ColumnData::I8(Vec::new()),
        "i2" => ColumnData::I16(Vec::new()),
        "i4" => ColumnData::I32(Vec::new()),
        "i8" => ColumnData::I64(Vec::new()),
        "u1" => ColumnData::U8(Vec::new()),
        "u2" => ColumnData::U16(Vec::new()),
        "u4" => ColumnData::U32(Vec::new()),
        "u8" => ColumnData::U64(Vec::new()),
        "f4" => ColumnData::F32(Vec::new()),
        "f8" => ColumnData::F64(Vec::new()),
        other => {
            return Err(Error::Codec(format!(
            "table column dtype '{other}' unsupported (numpy codes i1/i2/i4/i8 u1/u2/u4/u8 f4/f8)"
        )))
        }
    })
}

fn ze(e: impl std::fmt::Display) -> Error {
    Error::Codec(e.to_string())
}

/// A fresh Vortex runtime + session with the default encodings registered. The session needs a
/// runtime handle (`CurrentThreadRuntime`, no tokio) or async IO panics.
fn runtime_session() -> (CurrentThreadRuntime, VortexSession) {
    let rt = CurrentThreadRuntime::new();
    let s = VortexSession::empty()
        .with::<ArraySession>()
        .with::<LayoutSession>()
        .with::<ScalarFnSession>()
        .with::<RuntimeSession>()
        .with_handle(rt.handle());
    register_default_encodings(&s);
    (rt, s)
}

/// Validate that `data` is encodable under `spec`: same column count, names, dtypes, and every
/// column the same length == `rows`.
fn validate(spec: &TableSpec, data: &TableData) -> Result<()> {
    if data.len() != spec.columns.len() {
        return Err(Error::Codec(format!(
            "table has {} columns, spec declares {}",
            data.len(),
            spec.columns.len()
        )));
    }
    for (i, (col, (name, cd))) in spec.columns.iter().zip(data).enumerate() {
        if &col.name != name {
            return Err(Error::Codec(format!(
                "column {i}: name '{name}' != spec '{}'",
                col.name
            )));
        }
        if col.dtype != cd.numpy_code() {
            return Err(Error::Codec(format!(
                "column '{name}': dtype '{}' != spec '{}'",
                cd.numpy_code(),
                col.dtype
            )));
        }
        if cd.len() as u64 != spec.rows {
            return Err(Error::Codec(format!(
                "column '{name}': {} rows != spec rows {}",
                cd.len(),
                spec.rows
            )));
        }
    }
    Ok(())
}

/// The fixed table row-group size: a table block is **always** written as a chunked Vortex file with
/// this many rows per group (the last group is the remainder). A power-of-two constant so it's part
/// of the format contract, never a writer knob — one encoder serves batch *and* streaming, and a
/// >RAM producer can flush one row-group at a time (ADR-0026).
pub const ROWS_PER_GROUP: usize = 1 << 16; // 65_536

/// Encode columns into the deterministic table-block payload bytes for `spec`.
///
/// The table is written as a **chunked** Vortex file: the columns are sliced into fixed
/// [`ROWS_PER_GROUP`] row-groups and streamed as chunks. The grid is fixed, so the bytes are a pure
/// function of the data (batch and streaming produce the *same* bytes — there is one encoder).
///
/// **ALP float-encoding is excluded** from the write strategy: Vortex's ALP codec searches for a
/// float exponent via float arithmetic, whose result varies with the *build profile's* float codegen
/// (opt-level / FMA contraction), so the same float columns would encode to different bytes under
/// different compilers — fatal for a content-addressed format. With ALP excluded, floats fall back to
/// flat `Primitive` (raw little-endian, codegen-independent) while integer encodings (Sequence/FoR/…,
/// chosen by exact integer math) remain. The payload is then a pure function of the logical data
/// (cross-environment deterministic).
pub fn encode(spec: &TableSpec, data: &TableData) -> Result<Vec<u8>> {
    validate(spec, data)?;
    let (rt, s) = runtime_session();
    let fields: Vec<(&str, ArrayRef)> = data
        .iter()
        .map(|(name, cd)| (name.as_str(), cd.to_vortex()))
        .collect();
    let full = StructArray::from_fields(&fields).map_err(ze)?.into_array();
    // Slice into fixed row-groups (≥ 1 chunk even when empty) → a chunked Vortex layout.
    let rows = spec.rows as usize;
    let n_groups = rows.div_ceil(ROWS_PER_GROUP).max(1);
    let chunks: Vec<ArrayRef> = (0..n_groups)
        .map(|g| {
            let start = g * ROWS_PER_GROUP;
            let end = ((g + 1) * ROWS_PER_GROUP).min(rows);
            full.slice(start..end).map_err(ze)
        })
        .collect::<Result<_>>()?;
    let chunked = ChunkedArray::from_iter(chunks).into_array();
    // Exclude the ALP float schemes from the compressor so it never *chooses* them; floats then use
    // the deterministic Pco/flat schemes. Integer schemes (chosen by exact integer math) are kept.
    let compressor =
        BtrBlocksCompressorBuilder::default().exclude_schemes([ALPScheme.id(), ALPRDScheme.id()]);
    let strategy = WriteStrategyBuilder::default()
        .with_btrblocks_builder(compressor)
        .build();
    let mut buf = ByteBufferMut::empty();
    rt.block_on(
        s.write_options()
            .with_strategy(strategy)
            .write(&mut buf, chunked.to_array_stream()),
    )
    .map_err(ze)?;
    Ok(buf.freeze().to_vec())
}

/// **Bounded-memory / >RAM variant of [`encode`]**: consume row-group [`TableData`] chunks from a
/// *lazy* iterator (each ≤ [`ROWS_PER_GROUP`] rows) and write the chunked Vortex bytes **without ever
/// holding the whole table** — the DAQ / streaming-compaction path. The iterator is pulled one chunk
/// at a time as the writer consumes it (`ArrayIteratorAdapter` → `into_array_stream`), so a producer
/// that reads one row-group fragment at a time stays at ~one-group RAM.
///
/// Feeding the row-groups in fixed-grid order yields bytes **byte-identical to [`encode`]** of the
/// concatenation — there is one logical encoder, so streaming-then-compact == batch (tested by
/// `encode_streaming_matches_batch_encode`).
pub fn encode_streaming<I>(spec: &TableSpec, groups: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = TableData>,
    I::IntoIter: Send + 'static,
{
    let (rt, s) = runtime_session();
    // The struct dtype, taken from an empty struct of the declared columns.
    let mut empty: TableData = Vec::with_capacity(spec.columns.len());
    for c in &spec.columns {
        empty.push((c.name.clone(), empty_column(&c.dtype)?));
    }
    let efields: Vec<(&str, ArrayRef)> = empty
        .iter()
        .map(|(n, c)| (n.as_str(), c.to_vortex()))
        .collect();
    let dtype = StructArray::from_fields(&efields)
        .map_err(ze)?
        .into_array()
        .dtype()
        .clone();

    // Lazily turn each row-group into a Vortex StructArray chunk (one in flight at a time).
    let chunk_iter = groups.into_iter().map(|td| {
        let fields: Vec<(&str, ArrayRef)> = td
            .iter()
            .map(|(n, c)| (n.as_str(), c.to_vortex()))
            .collect();
        StructArray::from_fields(&fields).map(|s| s.into_array())
    });
    let array_iter = ArrayIteratorAdapter::new(dtype, chunk_iter);

    let compressor =
        BtrBlocksCompressorBuilder::default().exclude_schemes([ALPScheme.id(), ALPRDScheme.id()]);
    let strategy = WriteStrategyBuilder::default()
        .with_btrblocks_builder(compressor)
        .build();
    let mut buf = ByteBufferMut::empty();
    rt.block_on(
        s.write_options()
            .with_strategy(strategy)
            .write(&mut buf, array_iter.into_array_stream()),
    )
    .map_err(ze)?;
    Ok(buf.freeze().to_vec())
}

/// Append a decoded (canonicalized) Vortex column's values onto the matching output (bit-exact).
fn extend_column(col: &mut ColumnData, prim: &PrimitiveArray) {
    match col {
        ColumnData::I8(v) => v.extend_from_slice(prim.as_slice::<i8>()),
        ColumnData::I16(v) => v.extend_from_slice(prim.as_slice::<i16>()),
        ColumnData::I32(v) => v.extend_from_slice(prim.as_slice::<i32>()),
        ColumnData::I64(v) => v.extend_from_slice(prim.as_slice::<i64>()),
        ColumnData::U8(v) => v.extend_from_slice(prim.as_slice::<u8>()),
        ColumnData::U16(v) => v.extend_from_slice(prim.as_slice::<u16>()),
        ColumnData::U32(v) => v.extend_from_slice(prim.as_slice::<u32>()),
        ColumnData::U64(v) => v.extend_from_slice(prim.as_slice::<u64>()),
        ColumnData::F32(v) => v.extend_from_slice(prim.as_slice::<f32>()),
        ColumnData::F64(v) => v.extend_from_slice(prim.as_slice::<f64>()),
    }
}

/// Decode the whole table from a block payload (inverse of [`encode`]).
pub fn decode(spec: &TableSpec, blob: &[u8]) -> Result<TableData> {
    let (rt, s) = runtime_session();
    // Accumulators, one per declared column (column order == struct field order on write).
    let mut cols: Vec<ColumnData> = spec
        .columns
        .iter()
        .map(|c| empty_column(&c.dtype))
        .collect::<Result<_>>()?;

    let mut ctx = s.create_execution_ctx();
    rt.block_on(async {
        let stream = s
            .open_options()
            .open_buffer(ByteBuffer::copy_from(blob))
            .map_err(ze)?
            .scan()
            .map_err(ze)?
            .into_array_stream()
            .map_err(ze)?;
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let st: StructArray = chunk.map_err(ze)?.execute(&mut ctx).map_err(ze)?;
            for (i, col) in cols.iter_mut().enumerate() {
                let prim: PrimitiveArray =
                    st.unmasked_field(i).clone().execute(&mut ctx).map_err(ze)?;
                extend_column(col, &prim);
            }
        }
        Ok::<(), Error>(())
    })?;

    Ok(spec
        .columns
        .iter()
        .map(|c| c.name.clone())
        .zip(cols)
        .collect())
}

/// Decode a SINGLE column from a table block via Vortex **projection** — the scan reads only that
/// column's layout segments, so it doesn't materialise the whole table (the columnar-take win;
/// cf. Parquet/ROOT column projection in the #143 ecosystem bench). Bit-exact with [`decode`]'s
/// matching column.
pub fn decode_column(spec: &TableSpec, blob: &[u8], name: &str) -> Result<ColumnData> {
    let col = spec
        .columns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| Error::Codec(format!("table has no column '{name}'")))?;
    let (rt, s) = runtime_session();
    let mut out = empty_column(&col.dtype)?;
    let mut ctx = s.create_execution_ctx();
    rt.block_on(async {
        let stream = s
            .open_options()
            .open_buffer(ByteBuffer::copy_from(blob))
            .map_err(ze)?
            .scan()
            .map_err(ze)?
            .with_projection(select([name], root())) // only this field is scanned
            .into_array_stream()
            .map_err(ze)?;
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let st: StructArray = chunk.map_err(ze)?.execute(&mut ctx).map_err(ze)?;
            let prim: PrimitiveArray =
                st.unmasked_field(0).clone().execute(&mut ctx).map_err(ze)?;
            extend_column(&mut out, &prim);
        }
        Ok::<(), Error>(())
    })?;
    Ok(out)
}

/// Build the `{hash, stats}` chunk-index (ADR-0028 §3) for a table block, splitting on the **same**
/// fixed [`ROWS_PER_GROUP`] row-groups [`encode`] uses. Each entry carries the row-group's content digest
/// (BLAKE3 over the group's little-endian column bytes — recomputable from the decoded group, independent
/// of the Vortex byte layout) and the chunk statistics of `stat_column` (which must be an integer column,
/// see [`ColumnData::as_i64`]). Other columns still feed each group's digest; only the stats come from
/// `stat_column`. `index.root()` is the block's sub-block Merkle root (ADR-0028 §1), and `index.prune()`
/// skips row-groups a ranged read can't hit. Per #221-B, the *index* leaf may later be finer than
/// `ROWS_PER_GROUP`; this first wiring uses the encoder's row-groups 1:1.
pub fn table_chunk_index(
    spec: &TableSpec,
    data: &TableData,
    stat_column: &str,
) -> Result<ChunkIndex> {
    validate(spec, data)?;
    let stat_vals = data
        .iter()
        .find(|(name, _)| name == stat_column)
        .ok_or_else(|| Error::Codec(format!("table has no column '{stat_column}'")))?
        .1
        .as_i64()
        .ok_or_else(|| {
            Error::Codec(format!(
                "column '{stat_column}' is not an integer column for stats"
            ))
        })?;
    let rows = data.first().map(|(_, c)| c.len()).unwrap_or(0);
    let n_groups = rows.div_ceil(ROWS_PER_GROUP).max(1);
    let mut idx = ChunkIndex::new();
    for g in 0..n_groups {
        let start = g * ROWS_PER_GROUP;
        let end = ((g + 1) * ROWS_PER_GROUP).min(rows);
        // group content digest = every column's LE bytes for [start, end), in column order.
        let mut bytes = Vec::new();
        for (_, col) in data.iter() {
            bytes.extend_from_slice(&col.slice(start, end).to_le_bytes());
        }
        idx.push_entry(
            digest(&bytes),
            ChunkStats::from_values(&stat_vals[start..end]),
        );
    }
    Ok(idx)
}

/// Encode a table block and produce both the digested [`BlockRef`] (digest over the real Vortex
/// payload bytes) and the [`BlockPayload`] to pack.
pub fn table_block(
    name: &str,
    spec: &TableSpec,
    data: &TableData,
) -> Result<(BlockRef, BlockPayload)> {
    let payload = encode(spec, data)?;
    let digest = tessera_core::hash::digest(&payload);
    let block_ref = BlockRef {
        name: name.to_string(),
        kind: BlockKind::Table,
        digest: Some(digest),
        spec: serde_json::to_value(spec)?,
    };
    Ok((block_ref, BlockPayload::new(name, payload)))
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

    /// A table spec + data with every supported dtype as a column.
    fn all_dtype_table(rows: usize) -> (TableSpec, TableData) {
        let data: TableData = vec![
            (
                "i1".into(),
                ColumnData::I8((0..rows).map(|k| (k % 128) as i8 - 64).collect()),
            ),
            (
                "i2".into(),
                ColumnData::I16((0..rows).map(|k| (k % 4096) as i16 - 1024).collect()),
            ),
            (
                "i4".into(),
                ColumnData::I32((0..rows).map(|k| k as i32 * 7 - 100).collect()),
            ),
            (
                "i8".into(),
                ColumnData::I64((0..rows).map(|k| k as i64 * 1_000_003).collect()),
            ),
            (
                "u1".into(),
                ColumnData::U8((0..rows).map(|k| k as u8).collect()),
            ),
            (
                "u2".into(),
                ColumnData::U16((0..rows).map(|k| (k * 7) as u16).collect()),
            ),
            (
                "u4".into(),
                ColumnData::U32((0..rows).map(|k| (k * 999) as u32).collect()),
            ),
            (
                "u8".into(),
                ColumnData::U64((0..rows).map(|k| (k as u64) << 33).collect()),
            ),
            (
                "f4".into(),
                ColumnData::F32((0..rows).map(|k| k as f32 * 0.25 - 8.0).collect()),
            ),
            (
                "f8".into(),
                ColumnData::F64((0..rows).map(|k| k as f64 * 1.5).collect()),
            ),
        ];
        let columns = data.iter().map(|(n, c)| col(n, c.numpy_code())).collect();
        let spec = TableSpec {
            columns,
            rows: rows as u64,
            row_index: None,
        };
        (spec, data)
    }

    #[test]
    fn column_from_le_bytes_inverts_to_le_bytes() {
        let cases = [
            ColumnData::I8(vec![-1, 0, 127]),
            ColumnData::U8(vec![1, 2, 255]),
            ColumnData::U16(vec![0, 513, 65535]),
            ColumnData::F32(vec![0.5, -0.0, f32::INFINITY]),
            ColumnData::I64(vec![-1, 1_000_003, i64::MIN]),
        ];
        for c in cases {
            let back = ColumnData::from_le_bytes(c.numpy_code(), &c.to_le_bytes()).unwrap();
            assert_eq!(back, c);
        }
        assert!(ColumnData::from_le_bytes("f4", &[0u8; 3]).is_err()); // not a multiple of width
    }

    #[test]
    fn encode_streaming_matches_batch_encode() {
        // The SSoT proof: feeding row-groups through the lazy streaming encoder produces bytes
        // byte-identical to a batch encode of the whole table → one encoder, streaming == batch.
        let rows = ROWS_PER_GROUP + 9000; // 2 groups + remainder
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
        let n = rows.div_ceil(ROWS_PER_GROUP);
        let groups: Vec<TableData> = (0..n)
            .map(|g| {
                let (st, en) = (g * ROWS_PER_GROUP, ((g + 1) * ROWS_PER_GROUP).min(rows));
                full.iter()
                    .map(|(name, c)| (name.clone(), c.slice(st, en)))
                    .collect()
            })
            .collect();
        let streamed = encode_streaming(&spec, groups).unwrap();
        let batch = encode(&spec, &full).unwrap();
        assert_eq!(
            streamed, batch,
            "streaming-then-compact != batch — SSoT broken"
        );
        assert_eq!(
            decode(&spec, &streamed).unwrap(),
            full,
            "streamed bytes must decode"
        );
    }

    #[test]
    fn multi_rowgroup_roundtrips_and_is_deterministic() {
        // > ROWS_PER_GROUP forces several chunks (here 2 groups + remainder).
        let rows = ROWS_PER_GROUP + 4242;
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "f4")],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        let data: TableData = vec![
            ("t".into(), ColumnData::U64((0..rows as u64).collect())),
            (
                "e".into(),
                ColumnData::F32((0..rows).map(|k| 511.0 + (k % 13) as f32).collect()),
            ),
        ];
        let blob = encode(&spec, &data).unwrap();
        assert_eq!(decode(&spec, &blob).unwrap(), data, "multi-chunk roundtrip");
        assert_eq!(
            encode(&spec, &data).unwrap(),
            blob,
            "multi-chunk non-deterministic"
        );
        // projection still works across chunks
        let ColumnData::F32(e) = &decode_column(&spec, &blob, "e").unwrap() else {
            panic!()
        };
        assert_eq!(e.len(), rows);
    }

    #[test]
    fn table_chunk_index_groups_stats_and_prunes() {
        let rows = ROWS_PER_GROUP + 4242; // 2 row-groups (one full + a remainder)
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "f4")],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        let data: TableData = vec![
            ("t".into(), ColumnData::U64((0..rows as u64).collect())), // monotonic → prunable
            (
                "e".into(),
                ColumnData::F32((0..rows).map(|k| (k % 7) as f32).collect()),
            ),
        ];
        let idx = table_chunk_index(&spec, &data, "t").unwrap();
        // one entry per encoder row-group
        assert_eq!(idx.len(), rows.div_ceil(ROWS_PER_GROUP));
        // stats roll up to the whole monotonic column [0, rows)
        let agg = idx.aggregate();
        assert_eq!(agg.count, rows as u64);
        assert_eq!(agg.min, Some(0));
        assert_eq!(agg.max, Some(rows as i64 - 1));
        // 't' is sorted, so a low/high value range keeps only the first/last group
        assert_eq!(idx.prune(0, 10), vec![0]);
        let last = idx.len() - 1;
        assert_eq!(idx.prune(rows as i64 - 1, rows as i64 - 1), vec![last]);
        // root = the sub-block MMR over the per-group digests; deterministic
        assert!(idx.root().starts_with("blake3:"));
        assert_eq!(
            table_chunk_index(&spec, &data, "t").unwrap().root(),
            idx.root()
        );
        // a float column can't supply integer stats
        assert!(table_chunk_index(&spec, &data, "e").is_err());
    }

    #[test]
    fn decode_column_projection_matches_full_decode() {
        let (spec, data) = all_dtype_table(257);
        let blob = encode(&spec, &data).unwrap();
        let full = decode(&spec, &blob).unwrap();
        // every column read via projection equals the same column from the full decode
        for (name, col) in &full {
            assert_eq!(
                &decode_column(&spec, &blob, name).unwrap(),
                col,
                "projection mismatch for column {name}"
            );
        }
        assert!(decode_column(&spec, &blob, "nope").is_err());
    }

    #[test]
    fn roundtrip_every_dtype_and_deterministic() {
        let (spec, data) = all_dtype_table(257); // odd, multi-of-nothing
        let blob = encode(&spec, &data).unwrap();
        let back = decode(&spec, &blob).unwrap();
        assert_eq!(back, data, "table roundtrip mismatch");
        assert_eq!(
            encode(&spec, &data).unwrap(),
            blob,
            "table non-deterministic"
        );
    }

    #[test]
    fn float_bit_patterns_survive_exactly() {
        let mut f4: Vec<f32> = (0..32).map(|k| k as f32).collect();
        for (j, v) in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            0.0,
            f32::MIN_POSITIVE,
            f32::MIN_POSITIVE / 2.0,
            f32::MIN,
        ]
        .into_iter()
        .enumerate()
        {
            f4[j] = v;
        }
        let mut f8: Vec<f64> = (0..32).map(|k| k as f64).collect();
        for (j, v) in [f64::NAN, f64::NEG_INFINITY, -0.0, f64::MIN_POSITIVE / 2.0]
            .into_iter()
            .enumerate()
        {
            f8[j] = v;
        }
        let data: TableData = vec![
            ("a".into(), ColumnData::F32(f4.clone())),
            ("b".into(), ColumnData::F64(f8.clone())),
        ];
        let spec = TableSpec {
            columns: vec![col("a", "f4"), col("b", "f8")],
            rows: 32,
            row_index: None,
        };
        let back = decode(&spec, &encode(&spec, &data).unwrap()).unwrap();
        let ColumnData::F32(ga) = &back[0].1 else {
            panic!()
        };
        let ColumnData::F64(gb) = &back[1].1 else {
            panic!()
        };
        for (a, b) in ga.iter().zip(&f4) {
            assert_eq!(a.to_bits(), b.to_bits(), "f32 bit pattern diverged");
        }
        for (a, b) in gb.iter().zip(&f8) {
            assert_eq!(a.to_bits(), b.to_bits(), "f64 bit pattern diverged");
        }
    }

    #[test]
    fn table_block_digest_is_over_real_payload() {
        let (spec, data) = all_dtype_table(16);
        let (block_ref, payload) = table_block("events", &spec, &data).unwrap();
        assert_eq!(
            block_ref.digest.unwrap(),
            tessera_core::hash::digest(&payload.bytes)
        );
        assert_eq!(decode(&spec, &payload.bytes).unwrap(), data);
    }

    #[test]
    fn rejects_dtype_name_and_len_mismatches() {
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "f4")],
            rows: 4,
            row_index: None,
        };
        // wrong dtype for column 'e'
        let bad_dtype: TableData = vec![
            ("t".into(), ColumnData::U64(vec![0; 4])),
            ("e".into(), ColumnData::I32(vec![0; 4])),
        ];
        assert!(matches!(encode(&spec, &bad_dtype), Err(Error::Codec(_))));
        // wrong length
        let bad_len: TableData = vec![
            ("t".into(), ColumnData::U64(vec![0; 4])),
            ("e".into(), ColumnData::F32(vec![0.0; 3])),
        ];
        assert!(matches!(encode(&spec, &bad_len), Err(Error::Codec(_))));
        // wrong name
        let bad_name: TableData = vec![
            ("x".into(), ColumnData::U64(vec![0; 4])),
            ("e".into(), ColumnData::F32(vec![0.0; 4])),
        ];
        assert!(matches!(encode(&spec, &bad_name), Err(Error::Codec(_))));
    }
}
