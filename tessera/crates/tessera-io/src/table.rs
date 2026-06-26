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
use tessera_core::{Error, Result};
use vortex_array::arrays::struct_::StructArrayExt;
use vortex_array::arrays::{PrimitiveArray, StructArray};
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

/// Encode columns into the deterministic table-block payload bytes for `spec`.
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
    let st = StructArray::from_fields(&fields).map_err(ze)?;
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
            .write(&mut buf, st.into_array().to_array_stream()),
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
