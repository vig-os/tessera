//! Table block backend — columnar storage as a deterministic **Vortex** file (the settled table
//! backend: smallest + O(1) random-take + filter-pushdown + zero-copy Arrow→DuckDB, spike
//! S0/S4/S7/S10/S11). The real codec behind a [`TableSpec`] block: a set of typed columns ⇄ the
//! exact bytes stored at `blocks/<name>` in a `.tsra`, and back.
//!
//! Like [`crate::array`], the payload is one self-contained, **byte-deterministic** blob (verified:
//! the Vortex 0.75 file writer produces identical bytes for identical input — the writer-determinism
//! release gate), digested over the encoded bytes. Column dtypes use the fd5 numpy-style codes
//! (`i1/i2/i4/i8`, `u1/u2/u4/u8`, `f4/f8`) carried in [`tessera_core::block::table::Column`].
//!
//! # Reading — performance & the intended access pattern
//!
//! **These are Vortex-native reads and they parallelise.** [`decode`] /
//! [`decode_projected`] drive the scan on a per-thread multi-core worker pool
//! (`READ_RT`) so segment I/O + decode fan out across cores — do **not**
//! reach for the bare single-threaded runtime and hand-roll a scan loop; that
//! path is single-core and will read ~4× slower than a mature row store, which is
//! a misuse artefact, not a property of the format.
//!
//! Match the read to the format's shape:
//! - **Project** — ask only for the columns you need ([`decode_projected`] /
//!   [`decode_column`]); Vortex reads just those columns' layout segments.
//! - **Full-materialise-to-`Vec<struct>` is the slow path on purpose.** The
//!   fast, intended consumption is the columnar/zero-copy one (project + filter,
//!   hand the canonical arrays to Arrow/DuckDB) — not decompressing every row into
//!   host structs. A `decode`-everything-then-iterate bench measures the one
//!   access pattern a columnar store is worst at.
//!
//! (Context: this guidance was added after a good-faith integrator copied
//! `runtime_session`'s single-thread runtime into a hand-rolled loop, benched
//! full-materialise, and wrongly concluded "Vortex decode is slow." The runtime
//! choice + intended access pattern were the missing signposts.)

use futures::StreamExt;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::chunk_index::{ChunkIndex, ChunkStats};
use tessera_core::hash::digest;
use tessera_core::{Error, Result};
use vortex_array::accessor::ArrayAccessor;
use vortex_array::arrays::struct_::StructArrayExt;
use vortex_array::arrays::{BoolArray, ChunkedArray, PrimitiveArray, StructArray, VarBinViewArray};
use vortex_array::expr::{root, select};
use vortex_array::iter::{ArrayIteratorAdapter, ArrayIteratorExt};
use vortex_array::scalar_fn::session::ScalarFnSession;
use vortex_array::session::ArraySession;
use vortex_array::ExecutionCtx;
use vortex_array::{ArrayRef, IntoArray, VortexSessionExecute};
use vortex_btrblocks::schemes::float::{ALPRDScheme, ALPScheme, PcoScheme};
use vortex_btrblocks::{BtrBlocksCompressorBuilder, SchemeExt};
use vortex_buffer::{Buffer, ByteBuffer, ByteBufferMut};
use vortex_file::{
    register_default_encodings, OpenOptionsSessionExt, WriteOptionsSessionExt, WriteStrategyBuilder,
};
use vortex_io::runtime::current::{CurrentThreadRuntime, CurrentThreadWorkerPool};
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
    /// Boolean column (dtype code `b1`). Bit-packed on the wire (Vortex
    /// `BoolArray`); this in-memory form is a plain `Vec<bool>`.
    Bool(Vec<bool>),
    /// UTF-8 string column (dtype code `str`). Genuine variable/high-cardinality
    /// text; Vortex picks FSST/dictionary automatically for low-cardinality
    /// repeats. (For *known* small enums prefer an integer code column + a
    /// categorical descriptor — a schema choice, not a `ColumnData` variant.)
    Utf8(Vec<String>),
    /// A column whose values may be **NULL** (#330), paired with a validity mask:
    /// `validity[i] == true` ⇔ `values[i]` is present.
    ///
    /// A *wrapper* rather than a validity mask on every variant, deliberately: nullability is
    /// orthogonal to dtype, so folding it into each variant would square the enum and force every
    /// existing match arm to handle a mask it does not care about. Wrapping also keeps the
    /// non-nullable encode path byte-identical to pre-#330 output, which is what lets the
    /// committed conformance corpus keep its goldens.
    ///
    /// NULL is *not* a float sentinel: `NaN` is a legitimate measured value, and integer columns
    /// have no sentinel at all. The mask is the only honest representation.
    ///
    /// `values` is never itself `Nullable` — `validate_nullable` rejects nesting.
    ///
    /// **Values under a NULL are not preserved.** Encoding normalises every masked-out slot to the
    /// dtype default, so a decoded column carries `0` (or `0.0`) wherever `validity` is false,
    /// whatever the producer had there. This is deliberate: it makes the encoded bytes — and
    /// therefore `content_hash` — independent of whatever happened to sit under a null, which the
    /// format's determinism guarantee requires. Compare only the slots where `validity` is true.
    Nullable {
        values: Box<ColumnData>,
        validity: Vec<bool>,
    },
}

/// Bit-pack a validity mask LSB-first, 8 flags per byte — the on-wire/in-fragment form.
/// `ceil(len/8)` bytes; trailing bits in the last byte are zero.
pub(crate) fn pack_validity(validity: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; validity.len().div_ceil(8)];
    for (i, &v) in validity.iter().enumerate() {
        if v {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// Inverse of [`pack_validity`] for exactly `n` flags (the trailing padding bits are ignored).
pub(crate) fn unpack_validity(bytes: &[u8], n: usize) -> Vec<bool> {
    (0..n)
        .map(|i| bytes.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0))
        .collect()
}

/// Bytes a packed validity mask of `n` flags occupies.
pub(crate) fn validity_pack_len(n: usize) -> usize {
    n.div_ceil(8)
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
            ColumnData::Bool(_) => "b1",
            ColumnData::Utf8(_) => "str",
            ColumnData::Nullable { values, .. } => values.numpy_code(),
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
            ColumnData::Bool(v) => v.len(),
            ColumnData::Utf8(v) => v.len(),
            ColumnData::Nullable { values, .. } => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reserve capacity for exactly `additional` more rows in the backing `Vec` — the
    /// accumulator pre-size used by the slab decoders. Lives here (rather than as a
    /// per-variant `match` at each call site) so adding a variant can't leave a downstream
    /// pre-size loop silently unhandled.
    pub fn reserve_exact(&mut self, additional: usize) {
        match self {
            ColumnData::I8(v) => v.reserve_exact(additional),
            ColumnData::I16(v) => v.reserve_exact(additional),
            ColumnData::I32(v) => v.reserve_exact(additional),
            ColumnData::I64(v) => v.reserve_exact(additional),
            ColumnData::U8(v) => v.reserve_exact(additional),
            ColumnData::U16(v) => v.reserve_exact(additional),
            ColumnData::U32(v) => v.reserve_exact(additional),
            ColumnData::U64(v) => v.reserve_exact(additional),
            ColumnData::F32(v) => v.reserve_exact(additional),
            ColumnData::F64(v) => v.reserve_exact(additional),
            ColumnData::Bool(v) => v.reserve_exact(additional),
            ColumnData::Utf8(v) => v.reserve_exact(additional),
            ColumnData::Nullable { values, validity } => {
                values.reserve_exact(additional);
                validity.reserve_exact(additional);
            }
        }
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
            ColumnData::F32(_) | ColumnData::F64(_) | ColumnData::Bool(_) | ColumnData::Utf8(_) => {
                None
            }
            // NULLs are SKIPPED, not coerced: a chunk min/max must describe the values that
            // are actually present, or pruning would exclude chunks that do contain matches.
            ColumnData::Nullable { values, validity } => values.as_i64().map(|all| {
                all.into_iter()
                    .zip(validity)
                    .filter_map(|(x, &ok)| ok.then_some(x))
                    .collect()
            }),
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
            ColumnData::Bool(v) => v.iter().map(|&b| b as u8).collect(),
            // length-prefixed: [u32 LE len | utf8 bytes] per string.
            ColumnData::Utf8(v) => {
                let mut out = Vec::new();
                for s in v {
                    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
                out
            }
            // values ‖ bit-packed validity. The mask is a fixed ceil(rows/8) trailer, so a
            // reader that knows the row count can split it off without extra framing.
            ColumnData::Nullable { values, validity } => {
                let mut out = values.to_le_bytes();
                out.extend_from_slice(&pack_validity(validity));
                out
            }
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
            "b1" => ColumnData::Bool(bytes.iter().map(|&b| b != 0).collect()),
            // inverse of the Utf8 length-prefixed encoding: [u32 LE len | bytes]*.
            "str" => {
                let mut v = Vec::new();
                let mut i = 0usize;
                while i + 4 <= bytes.len() {
                    let len =
                        u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
                            as usize;
                    i += 4;
                    let end = i
                        .checked_add(len)
                        .filter(|&e| e <= bytes.len())
                        .ok_or_else(|| {
                            Error::Codec("truncated utf8 length-prefixed column".to_string())
                        })?;
                    v.push(String::from_utf8_lossy(&bytes[i..end]).into_owned());
                    i = end;
                }
                ColumnData::Utf8(v)
            }
            other => {
                return Err(Error::Codec(format!(
                    "unsupported column dtype code '{other}'"
                )))
            }
        })
    }

    /// An empty column of the shape `col` declares — wrapping in [`ColumnData::Nullable`] when the
    /// column is nullable. The nullable-aware counterpart of `from_le_bytes(dtype, &[])`, which
    /// only ever yields a bare column and so would silently drop nullability in the staging paths.
    pub fn empty_for(col: &Column) -> Result<ColumnData> {
        let values = ColumnData::from_le_bytes(&col.dtype, &[])?;
        Ok(if col.nullable {
            ColumnData::Nullable {
                values: Box::new(values),
                validity: Vec::new(),
            }
        } else {
            values
        })
    }

    /// Decode a column from its flat LE bytes given the declaring [`Column`] and its row count —
    /// the inverse of [`Self::to_le_bytes`] for both nullable and non-nullable columns.
    ///
    /// For a nullable column the buffer is `values ‖ packed_validity`, and the mask is a fixed
    /// `ceil(rows/8)` trailer, so it is split off the END. `n_rows` is required because the mask
    /// length cannot be derived from the buffer alone for a variable-width payload.
    pub fn from_column_bytes(col: &Column, bytes: &[u8], n_rows: usize) -> Result<ColumnData> {
        if !col.nullable {
            return ColumnData::from_le_bytes(&col.dtype, bytes);
        }
        let mask_len = validity_pack_len(n_rows);
        let split = bytes.len().checked_sub(mask_len).ok_or_else(|| {
            Error::Codec(format!(
                "column '{}': {} bytes is too short for a {n_rows}-row validity mask",
                col.name,
                bytes.len()
            ))
        })?;
        let values = ColumnData::from_le_bytes(&col.dtype, &bytes[..split])?;
        Ok(ColumnData::Nullable {
            values: Box::new(values),
            validity: unpack_validity(&bytes[split..], n_rows),
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
            ColumnData::Bool(v) => sl!(v, Bool),
            ColumnData::Utf8(v) => sl!(v, Utf8),
            ColumnData::Nullable { values, validity } => ColumnData::Nullable {
                values: Box::new(values.slice(start, end)),
                validity: validity[start..end].to_vec(),
            },
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
            ColumnData::Bool(v) => ext!(v, Bool),
            ColumnData::Utf8(v) => ext!(v, Utf8),
            ColumnData::Nullable { values, validity } => match other {
                ColumnData::Nullable {
                    values: ov,
                    validity: ovd,
                } => {
                    values.extend(ov)?;
                    validity.extend_from_slice(ovd);
                }
                // Extending a nullable column with a non-nullable one would silently invent
                // validity for the appended rows; require both sides to agree.
                _ => {
                    return Err(Error::Codec(format!(
                        "extend: dtype {} (non-nullable) != {} (nullable)",
                        other.numpy_code(),
                        self.numpy_code()
                    )))
                }
            },
        }
        Ok(())
    }

    /// Bytes per element of a **fixed-width** numpy dtype code (`i1`=1 … `f8`=8, `b1`=1).
    ///
    /// `b1` is 1 byte per value in this LE form (see [`Self::to_le_bytes`] — the *wire* form is
    /// bit-packed by Vortex, but the flat byte form is one byte per bool), so it is fixed-width
    /// like the numerics.
    ///
    /// `str` has **no** element size — it is length-prefixed and varies per value — so it is an
    /// error here by design. Callers that only need "is this a dtype we support?" must use
    /// [`Self::validate_dtype`]; using this function for that question silently excludes every
    /// variable-width column.
    pub fn dtype_size(code: &str) -> Result<usize> {
        Ok(match code {
            "i1" | "u1" | "b1" => 1,
            "i2" | "u2" => 2,
            "i4" | "u4" | "f4" => 4,
            "i8" | "u8" | "f8" => 8,
            "str" => {
                return Err(Error::Codec(
                    "dtype 'str' is variable-width and has no element size".to_string(),
                ))
            }
            other => return Err(Error::Codec(format!("unknown dtype code '{other}'"))),
        })
    }

    /// Whether `code` names a dtype [`ColumnData`] can represent — including the variable-width
    /// `str`. This is the correct front-door check for "can this column be written?"; it exists
    /// because [`Self::dtype_size`] was being used for that question, which rejected `b1`/`str`
    /// and so locked the boolean and string column types out of every streaming write path.
    pub fn validate_dtype(code: &str) -> Result<()> {
        match code {
            "i1" | "i2" | "i4" | "i8" | "u1" | "u2" | "u4" | "u8" | "f4" | "f8" | "b1" | "str" => {
                Ok(())
            }
            other => Err(Error::Codec(format!("unknown dtype code '{other}'"))),
        }
    }

    /// Whether `code` is fixed-width (every value occupies [`Self::dtype_size`] bytes).
    pub fn dtype_is_fixed_width(code: &str) -> bool {
        Self::dtype_size(code).is_ok()
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
            ColumnData::Bool(v) => v.iter().copied().collect::<BoolArray>().into_array(),
            ColumnData::Utf8(v) => {
                VarBinViewArray::from_iter_str(v.iter().map(|s| s.as_str())).into_array()
            }
            // Nullable maps onto Vortex's NATIVE validity, not a tessera-level sentinel, so the
            // compressor sees real nulls and the mask round-trips through the file format for
            // free. `from_option_iter` builds values+validity in one pass; an all-valid or
            // all-invalid mask collapses to a constant internally, costing no buffer.
            //
            // Scope: fixed-width numerics only — `validate_nullable` rejects a nullable Bool/Utf8
            // before any encode reaches here, so those arms are unreachable by construction
            // rather than by assumption.
            ColumnData::Nullable { values, validity } => {
                macro_rules! nullable {
                    ($v:expr) => {
                        PrimitiveArray::from_option_iter(
                            $v.iter().zip(validity).map(|(x, &ok)| ok.then_some(*x)),
                        )
                        .into_array()
                    };
                }
                match values.as_ref() {
                    ColumnData::I8(v) => nullable!(v),
                    ColumnData::I16(v) => nullable!(v),
                    ColumnData::I32(v) => nullable!(v),
                    ColumnData::I64(v) => nullable!(v),
                    ColumnData::U8(v) => nullable!(v),
                    ColumnData::U16(v) => nullable!(v),
                    ColumnData::U32(v) => nullable!(v),
                    ColumnData::U64(v) => nullable!(v),
                    ColumnData::F32(v) => nullable!(v),
                    ColumnData::F64(v) => nullable!(v),
                    other => {
                        // Not `unreachable!`: a future variant must fail loudly at the gate, not
                        // panic a writer. `validate_nullable` is the single door and names the
                        // dtype, so this only fires if someone bypasses it.
                        panic!(
                            "nullable '{}' columns are not supported yet — validate_nullable \
                             should have rejected this before encode",
                            other.numpy_code()
                        )
                    }
                }
            }
        }
    }
}

/// An ordered set of named columns — the decoded form of a table block (column order matches the
/// [`TableSpec`]). All columns have the same length (`rows`).
pub type TableData = Vec<(String, ColumnData)>;

/// An empty column of the dtype named by a numpy code (the decode accumulator).
fn empty_column(code: &str) -> Result<ColumnData> {
    empty_column_with_capacity(code, 0)
}

/// An empty accumulator column, pre-sized for `cap` rows.
///
/// **This is the dominant cost of a full-materialise read.** Profiling (`examples/read_profile`)
/// attributes a 2 M-row × 21-column [`decode`] as ~2 % layout/scan, ~17 % genuine Vortex
/// decompress, and **~81 % this host copy** — so the accumulators' growth policy, not the codec,
/// sets the read's speed. Growing from empty re-allocates + re-copies each column O(log n) times
/// (~2–3× the traffic of one pass); reserving the declared row count up front leaves exactly one
/// copy per row-group.
///
/// `cap` comes from the spec's *declared* `rows`, which is attacker-controlled for an untrusted
/// blob, so it is clamped to [`BLOCK_ROWS`] — the partitioning law's maximum rows in one table
/// block. A larger table still decodes correctly; it just resumes amortised growth past the clamp.
/// Pre-sized accumulator for the column `col` declares — the nullable-aware counterpart of
/// [`empty_column_with_capacity`].
///
/// The decode path MUST build accumulators from the `Column`, not from the dtype string alone: a
/// bare accumulator for a nullable column would take `extend_field`'s non-nullable branch, and the
/// validity mask would be dropped silently — the values would still decode, so nothing would fail
/// loudly. Both the values and the mask are reserved, since the mask grows per row too.
fn empty_column_for(col: &Column, cap: usize) -> Result<ColumnData> {
    let values = empty_column_with_capacity(&col.dtype, cap)?;
    Ok(if col.nullable {
        let mut validity = Vec::new();
        validity.reserve_exact(cap.min(BLOCK_ROWS));
        ColumnData::Nullable {
            values: Box::new(values),
            validity,
        }
    } else {
        values
    })
}

fn empty_column_with_capacity(code: &str, cap: usize) -> Result<ColumnData> {
    let cap = cap.min(BLOCK_ROWS);
    Ok(match code {
        "i1" => ColumnData::I8(Vec::with_capacity(cap)),
        "i2" => ColumnData::I16(Vec::with_capacity(cap)),
        "i4" => ColumnData::I32(Vec::with_capacity(cap)),
        "i8" => ColumnData::I64(Vec::with_capacity(cap)),
        "u1" => ColumnData::U8(Vec::with_capacity(cap)),
        "u2" => ColumnData::U16(Vec::with_capacity(cap)),
        "u4" => ColumnData::U32(Vec::with_capacity(cap)),
        "u8" => ColumnData::U64(Vec::with_capacity(cap)),
        "f4" => ColumnData::F32(Vec::with_capacity(cap)),
        "f8" => ColumnData::F64(Vec::with_capacity(cap)),
        "b1" => ColumnData::Bool(Vec::with_capacity(cap)),
        "str" => ColumnData::Utf8(Vec::with_capacity(cap)),
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

// The per-thread Vortex runtimes + sessions the read paths use. A session needs a runtime handle
// (`CurrentThreadRuntime`, no tokio) or async IO panics, so each entry pairs the two.
// (Plain comment, not a doc comment: rustdoc does not document macro invocations, and a `///` here
// is an `unused_doc_comments` error under `-D warnings`.)
/// A fresh, **independent** encoding-registered session bound to `rt`'s handle.
///
/// Sessions must never be shared across runtimes. `VortexSession` is `Arc`-backed,
/// so `clone().with_handle(..)` rebinds the *shared* state instead of producing an
/// independent session: caching one template and re-binding a handle per call lets a
/// short-lived runtime (`encode`/`decode_column`) leave every other holder — notably
/// the long-lived pooled [`READ_RT`] session — pointing at a dropped runtime. The next
/// pooled read then panics `Attempted to use a Handle after its runtime was dropped`.
/// Regression-tested by `short_lived_runtime_does_not_poison_pooled_session`.
///
/// Building per runtime instead of cloning a template costs nothing measurable: the
/// read win is the worker pool, not session reuse (`examples/read_profile` attributes
/// ~93 % of a read to the host copy; template caching moved nothing).
fn new_session(rt: &CurrentThreadRuntime) -> VortexSession {
    let s = VortexSession::empty()
        .with::<ArraySession>()
        .with::<LayoutSession>()
        .with::<ScalarFnSession>()
        .with::<RuntimeSession>();
    register_default_encodings(&s);
    s.with_handle(rt.handle())
}

thread_local! {
    /// Per-thread **pooled** read runtime: a `CurrentThreadWorkerPool` sized to
    /// available parallelism drives the scan's segment I/O + decode across all
    /// cores in the background while `block_on` awaits results. The bare
    /// `CurrentThreadRuntime` [`runtime_session`] uses is single-threaded — which
    /// is the actual read-throughput bottleneck, NOT the columnar decode. The
    /// pool + workers are spawned once per thread and kept alive here.
    static READ_RT: (CurrentThreadRuntime, CurrentThreadWorkerPool, VortexSession) = {
        let rt = CurrentThreadRuntime::new();
        let pool = rt.new_pool();
        pool.set_workers_to_available_parallelism();
        let s = new_session(&rt);
        (rt, pool, s)
    };
}

/// A fresh Vortex runtime + session with the default encodings registered. The session needs a
/// runtime handle (`CurrentThreadRuntime`, no tokio) or async IO panics.
///
/// This is the *single-threaded* runtime. Read paths should prefer [`with_read_session`], which
/// hands out the pooled one ([`READ_RT`]).
fn runtime_session() -> (CurrentThreadRuntime, VortexSession) {
    let rt = CurrentThreadRuntime::new();
    let s = new_session(&rt);
    (rt, s)
}

/// Run `f` with the per-thread pooled read runtime+session (see [`READ_RT`]).
fn with_read_session<R>(f: impl FnOnce(&CurrentThreadRuntime, &VortexSession) -> R) -> R {
    READ_RT.with(|(rt, _pool, s)| f(rt, s))
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
        validate_nullable(name, col.nullable, cd)?;
    }
    Ok(())
}

/// The single gate for nullable columns (#330): the spec flag and the data must agree, the mask
/// must cover exactly the values, nesting is rejected, and the currently-supported scope is
/// enforced here rather than assumed downstream.
///
/// Keeping all four checks in one place is deliberate — `to_vortex` relies on this having run, so
/// a caller that bypasses it gets a named panic instead of silently encoding a wrong mask.
fn validate_nullable(name: &str, spec_nullable: bool, cd: &ColumnData) -> Result<()> {
    match cd {
        ColumnData::Nullable { values, validity } => {
            if !spec_nullable {
                return Err(Error::Codec(format!(
                    "column '{name}': data carries a validity mask but the spec column is not \
                     marked nullable"
                )));
            }
            if values.len() != validity.len() {
                return Err(Error::Codec(format!(
                    "column '{name}': {} values but {} validity flags",
                    values.len(),
                    validity.len()
                )));
            }
            if matches!(values.as_ref(), ColumnData::Nullable { .. }) {
                return Err(Error::Codec(format!(
                    "column '{name}': nested nullable columns are not representable"
                )));
            }
            if matches!(values.as_ref(), ColumnData::Bool(_) | ColumnData::Utf8(_)) {
                return Err(Error::Codec(format!(
                    "column '{name}': nullable '{}' is not supported yet — nullability currently \
                     covers the fixed-width numeric dtypes only",
                    values.numpy_code()
                )));
            }
            Ok(())
        }
        _ if spec_nullable => Err(Error::Codec(format!(
            "column '{name}': spec column is nullable but the data carries no validity mask"
        ))),
        _ => Ok(()),
    }
}

/// The fixed table row-group size: a table block is **always** written as a chunked Vortex file with
/// this many rows per group (the last group is the remainder). A power-of-two constant so it's part
/// of the format contract, never a writer knob — one encoder serves batch *and* streaming, and a
/// >RAM producer can flush one row-group at a time (ADR-0026).
pub const ROWS_PER_GROUP: usize = 1 << 16; // 65_536

/// **Format-invariant** maximum rows per table block — partitions a >`BLOCK_ROWS` listmode product
/// across multiple `events_NNNN` blocks (ADR-0026). Picked at 64 × [`ROWS_PER_GROUP`] = `2^22`
/// (≈ 4.19 M rows ≈ a few hundred MiB encoded for typical PET schemas), big enough that the per-block
/// Vortex footer overhead stays trivial and small enough that one block fits comfortably in a worker's
/// RAM. Changing this is a **format-breaking** change (it shifts the per-block partition, which shifts
/// the per-block bytes, which shifts every `content_hash`). The compile-time assertion below makes
/// the SSoT explicit: every block is *exactly* 64 row-groups (or fewer for the trailing partial).
pub const BLOCK_ROWS: usize = 1 << 22; // 4_194_304 = 64 × ROWS_PER_GROUP

// `is_multiple_of` is not yet const-stable on `usize` (1.87 stabilised the method, not the const
// form), so the const assertion uses the integer `%` operator directly.
#[allow(clippy::manual_is_multiple_of)]
const _: () = assert!(
    BLOCK_ROWS % ROWS_PER_GROUP == 0,
    "BLOCK_ROWS must be a whole multiple of ROWS_PER_GROUP — one block = N full row-groups"
);

/// Row-groups per full table block ([`BLOCK_ROWS`] / [`ROWS_PER_GROUP`]) — the canonical fragment
/// count at which the multi-block sink closes a block. Derived from the format invariants so a
/// future tuner can never let them drift apart.
pub const ROW_GROUPS_PER_BLOCK: usize = BLOCK_ROWS / ROWS_PER_GROUP;

/// How many [`BLOCK_ROWS`]-sized blocks `rows` partitions into. A product with `rows == 0` still
/// yields ONE block (the metadata-bearing single `events` block — an empty table is one empty
/// row-group, see [`encode`]). For `rows > 0` it's `ceil(rows / BLOCK_ROWS)` — the trailing block
/// may be partial. **Format invariant**: shared by every ingest path so whole-file and streamed
/// agree on the partition (and therefore on the `content_hash`).
pub fn block_count(rows: u64) -> u64 {
    partition_blocks(rows, BLOCK_ROWS as u64)
}

/// Partition `rows` into ceil(rows / block_rows) blocks (`max(1)` so an empty table still has one
/// block). Pure helper extracted from [`block_count`] so the partition logic can be unit-tested at a
/// **small** `block_rows` (cheap CI) while production stays pinned at the [`BLOCK_ROWS`] format
/// invariant.
pub fn partition_blocks(rows: u64, block_rows: u64) -> u64 {
    let br = block_rows.max(1);
    rows.div_ceil(br).max(1)
}

/// Canonical name for the `idx`-th of `total` blocks under `prefix`. `total <= 1` → `prefix` (the
/// **small-stays-single** invariant — a ≤ [`BLOCK_ROWS`] product writes exactly one `events` block,
/// byte-identical to today's pre-partition layout). Otherwise `prefix_NNNN` (zero-padded to 4
/// digits, plenty of headroom for the realistic block-count range — up to 9999 blocks ≈ 41 G rows).
/// Shared by writer and reader so the manifest order is unambiguous.
pub fn block_name(prefix: &str, idx: u64, total: u64) -> String {
    if total <= 1 {
        prefix.to_string()
    } else {
        format!("{prefix}_{idx:04}")
    }
}

/// Encode columns into the deterministic table-block payload bytes for `spec`.
///
/// The deterministic table compressor, shared by the batch and streaming encoders so they stay
/// byte-identical (there is one encoder).
///
/// - **ALP / ALPRD excluded** — their exponent search runs float arithmetic whose result varies with
///   the build profile's float codegen (opt-level / FMA), so they'd encode the same column to
///   different bytes under different compilers — fatal for content-addressing.
/// - **Pco registered** via [`with_new_scheme`](BtrBlocksCompressorBuilder::with_new_scheme) (it is
///   `#[cfg(feature = "pco")]`-gated and *not* in `ALL_SCHEMES`, so it must be added explicitly).
///   Pco is a deterministic numeric codec — the same family that carries the array path, already
///   proven byte-reproducible x86==ARM by the cross-arch conformance gate — so continuous float
///   columns compress instead of falling through to flat/raw (issue #380). Integer schemes, chosen by
///   exact integer math, are untouched.
fn deterministic_table_compressor() -> BtrBlocksCompressorBuilder {
    BtrBlocksCompressorBuilder::default()
        .exclude_schemes([ALPScheme.id(), ALPRDScheme.id()])
        .with_new_scheme(&PcoScheme)
}

/// The table is written as a **chunked** Vortex file: the columns are sliced into fixed
/// [`ROWS_PER_GROUP`] row-groups and streamed as chunks. The grid is fixed, so the bytes are a pure
/// function of the data (batch and streaming produce the *same* bytes — there is one encoder).
///
/// **Float columns use Pco, not ALP** (see [`deterministic_table_compressor`]): Vortex's ALP codec
/// searches for a float exponent via float arithmetic, whose result varies with the *build profile's*
/// float codegen (opt-level / FMA contraction), so the same float columns would encode to different
/// bytes under different compilers — fatal for a content-addressed format. ALP is excluded; Pco
/// (a deterministic numeric codec, and the same family already used on the array path) is registered
/// in its place so continuous floats actually compress. Integer encodings (Sequence/FoR/…, chosen by
/// exact integer math) remain. The payload is a pure function of the logical data (cross-environment
/// deterministic — proven by the cross-arch conformance gate).
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
    let strategy = WriteStrategyBuilder::default()
        .with_btrblocks_builder(deterministic_table_compressor())
        .build();
    let mut buf = ByteBufferMut::empty();
    rt.block_on(
        s.write_options()
            .with_strategy(strategy)
            .write(&mut buf, chunked.to_array_stream()),
    )
    .map_err(ze)?;
    let payload = buf.freeze().to_vec();
    // Encode-path observability (SSoT for table bytes): raw→encoded size + ratio, so devs see the
    // achieved columnar compression on write. raw = Σ column widths × rows. Zero-cost when no subscriber.
    let raw: usize = data
        .iter()
        .map(|(_, c)| c.len() * ColumnData::dtype_size(c.numpy_code()).unwrap_or(0))
        .sum();
    let ratio = (raw as f64) / (payload.len().max(1) as f64);
    tracing::debug!(
        target: "tessera::encode",
        kind = "table",
        rows = spec.rows,
        columns = spec.columns.len(),
        raw_bytes = raw,
        encoded_bytes = payload.len(),
        ratio = ratio,
        "encoded table block"
    );
    Ok(payload)
}

/// Decode ONLY the rows at `rows` from a table block, via Vortex **row-index pushdown**.
///
/// `rows` are block-local indices; they must be sorted and unique, and the result is in that same
/// order. Only the segments covering the selected rows are read and decoded, so a scattered take
/// of a few rows does not pay for materialising the whole block — the random-access property the
/// table backend was chosen for.
///
/// Bit-exact with slicing the corresponding rows out of [`decode`]'s output.
pub fn decode_rows(spec: &TableSpec, blob: &[u8], rows: &[u64]) -> Result<TableData> {
    if let Some(w) = rows.windows(2).find(|w| w[0] >= w[1]) {
        return Err(Error::Codec(format!(
            "decode_rows: indices must be sorted and unique (got {} then {})",
            w[0], w[1]
        )));
    }
    if let Some(&last) = rows.last() {
        if last >= spec.rows {
            return Err(Error::Codec(format!(
                "decode_rows: index {last} is out of range for a {}-row table",
                spec.rows
            )));
        }
    }
    let mut cols: Vec<ColumnData> = spec
        .columns
        .iter()
        .map(|c| empty_column_for(c, rows.len()))
        .collect::<Result<_>>()?;
    if rows.is_empty() {
        return Ok(spec
            .columns
            .iter()
            .map(|c| c.name.clone())
            .zip(cols)
            .collect());
    }
    with_read_session(|rt, s| {
        let chunks = rt.block_on(scan_chunks_at(s, blob, None, Some(rows)))?;
        materialise(s, &chunks, &mut cols)
    })?;
    Ok(spec
        .columns
        .iter()
        .map(|c| c.name.clone())
        .zip(cols)
        .collect())
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

    let strategy = WriteStrategyBuilder::default()
        .with_btrblocks_builder(deterministic_table_compressor())
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
/// Execute one struct field into its declared `ColumnData` type and append it.
/// Numeric columns canonicalize to `PrimitiveArray`; `Bool` → `BoolArray`,
/// `Utf8` → `VarBinViewArray` (the compressor's chosen scheme is transparent
/// here — canonicalization undoes FSST/dict/bit-packing).
fn extend_field(col: &mut ColumnData, field: ArrayRef, ctx: &mut ExecutionCtx) -> Result<()> {
    match col {
        ColumnData::Bool(v) => {
            let b: BoolArray = field.execute(ctx).map_err(ze)?;
            v.extend(b.into_bit_buffer().iter());
        }
        ColumnData::Utf8(v) => {
            let s: VarBinViewArray = field.execute(ctx).map_err(ze)?;
            s.with_iterator(|it| {
                for opt in it {
                    v.push(
                        opt.map(|b| String::from_utf8_lossy(b).into_owned())
                            .unwrap_or_default(),
                    );
                }
            });
        }
        // Execute ONCE and take both halves from the same array: the values, and the validity
        // mask derived from Vortex's own nullability. Executing twice would decode the chunk twice
        // and (worse) could disagree.
        ColumnData::Nullable { values, validity } => {
            let prim: PrimitiveArray = field.execute(ctx).map_err(ze)?;
            let mask = prim
                .validity()
                .map_err(ze)?
                .execute_mask(prim.len(), ctx)
                .map_err(ze)?;
            validity.extend(mask.iter());
            extend_column(values, &prim);
        }
        _ => {
            let prim: PrimitiveArray = field.execute(ctx).map_err(ze)?;
            extend_column(col, &prim);
        }
    }
    Ok(())
}

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
        ColumnData::Bool(_) | ColumnData::Utf8(_) | ColumnData::Nullable { .. } => {
            unreachable!(
                "bool/utf8/nullable columns are handled in extend_field, not as primitives"
            )
        }
    }
}

/// Below this many values (rows × columns) the fan-out in [`materialise`] costs more in thread
/// spawns than it saves. Small tables stay exactly as serial as they were.
const PARALLEL_MATERIALISE_MIN_VALUES: usize = 1 << 20;

/// Decompress every column out of the already-scanned row-groups into the host accumulators, sizing
/// the fan-out against the whole machine — this is where a full-materialise read spends its time
/// (`examples/read_profile`: the field-decompress dominates; scan is ~2 %).
///
/// Routes to one of two parallel strategies, both **bit-identical** to a serial decode:
/// - **grid** ([`materialise_grid`]) for all-numeric tables — parallel over the (column × row-group)
///   grid, so the fan-out is *not* capped at the column count. Measured on an 88-core box:
///   a 4-column f64 table went 2.06× (804 → 1656 MB/s), reaching the same ~2 GB/s memory-bandwidth
///   ceiling as a wide table instead of stalling at 4 workers.
/// - **column** ([`materialise_with`]) as the fallback for Bool / Utf8 / Nullable shapes.
///
/// The driver hands over already-scanned `chunks`, whose fields are still *encoded* views into the
/// blob the caller already holds in memory — so this buys its parallelism without inflating the
/// resident set.
fn materialise(s: &VortexSession, chunks: &[StructArray], cols: &mut [ColumnData]) -> Result<()> {
    let ncols = cols.len();
    if ncols == 0 {
        return Ok(());
    }
    let values = chunks.iter().map(|c| c.len()).sum::<usize>() * ncols;
    if values < PARALLEL_MATERIALISE_MIN_VALUES {
        return materialise_with(s, chunks, cols, 1);
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // GRID path (#352): for numeric columns, parallelise the decompress over the (column ×
    // row-group) grid so the fan-out is NOT capped at the column count — a narrow (few-column)
    // table can still use every core by splitting each column's row-groups across workers. Each
    // (column, row-group) writes a DISJOINT offset slice of the pre-sized output, so there is no
    // lock and no concat, and the result is bit-identical to a serial decode. Measured: a 4-column
    // f64 table went from ~ncols-capped (stuck at 4 threads) to the memory-bandwidth ceiling.
    //
    // Bool / Utf8 / Nullable columns keep the column-parallel append path (`materialise_with`) — it
    // handles the variable-width / bit-packed / dual-vector cases and is already parallel across
    // columns; those shapes are rare in the wide-numeric listmode tables this grid targets.
    if is_grid_eligible(cols) {
        #[cfg(test)]
        GRID_DECODES.with(|c| c.set(c.get() + 1));
        materialise_grid(s, chunks, cols, cores)
    } else {
        materialise_with(s, chunks, cols, cores.min(ncols))
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only observability: per-thread count of how many times [`materialise`] routed to the
    /// numeric grid path. Tests gate the *routing* (a large numeric table must take the grid; a small
    /// or Bool/Utf8/Nullable one must not) without relying on timing — a non-flaky drift guard
    /// against the grid being silently disabled or the eligibility/threshold logic regressing. It is
    /// thread-local (not a global atomic) because the increment runs on the synchronous caller's
    /// thread, so it stays correct even though cargo runs tests in parallel.
    pub(crate) static GRID_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The grid decode path applies when every column is a plain fixed-width numeric vector (no
/// Bool/Utf8/Nullable) — the only shapes for which a disjoint offset-write is trivially correct.
fn is_grid_eligible(cols: &[ColumnData]) -> bool {
    cols.iter().all(|c| {
        matches!(
            c,
            ColumnData::I8(_)
                | ColumnData::I16(_)
                | ColumnData::I32(_)
                | ColumnData::I64(_)
                | ColumnData::U8(_)
                | ColumnData::U16(_)
                | ColumnData::U32(_)
                | ColumnData::U64(_)
                | ColumnData::F32(_)
                | ColumnData::F64(_)
        )
    })
}

/// [`materialise`] with the worker count pinned — the seam the equivalence test drives to prove
/// serial and parallel produce the same bytes.
fn materialise_with(
    s: &VortexSession,
    chunks: &[StructArray],
    cols: &mut [ColumnData],
    threads: usize,
) -> Result<()> {
    let ncols = cols.len();
    if ncols == 0 {
        return Ok(());
    }
    if threads <= 1 {
        let mut ctx = s.create_execution_ctx();
        for (i, col) in cols.iter_mut().enumerate() {
            for st in chunks {
                extend_field(col, st.unmasked_field(i).clone(), &mut ctx)?;
            }
        }
        return Ok(());
    }

    let per = ncols.div_ceil(threads);
    std::thread::scope(|sc| -> Result<()> {
        let handles: Vec<_> = cols
            .chunks_mut(per)
            .enumerate()
            .map(|(t, group)| {
                let s = s.clone(); // Arc-backed — the clone is the cheap part
                sc.spawn(move || -> Result<()> {
                    let mut ctx = s.create_execution_ctx();
                    for (j, col) in group.iter_mut().enumerate() {
                        let i = t * per + j;
                        for st in chunks {
                            extend_field(col, st.unmasked_field(i).clone(), &mut ctx)?;
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        for h in handles {
            h.join()
                .map_err(|_| Error::Codec("table decode worker panicked".into()))??;
        }
        Ok(())
    })
}

/// A boxed disjoint-slice write task for the grid path. Each captures one row-group's *encoded*
/// field plus the exact output sub-slice it fills, so tasks never alias and need no lock.
type GridTask<'a> = Box<dyn FnOnce(&mut ExecutionCtx) -> Result<()> + Send + 'a>;

/// Decode numeric columns across the **(column × row-group) grid** (see [`materialise`]). Pre-sizes
/// each output column to the full row count, then writes each row-group's decompressed values into
/// its own disjoint offset slice — so the fan-out ceiling is `columns × row-groups`, not `columns`.
/// Bit-identical to a serial decode: every slice is written exactly once from exactly one row-group.
// `clippy::uninit_vec`: the `reserve` + `set_len` below deliberately exposes uninitialised slots to
// skip a zero-fill; it is sound because the tasks cover and overwrite every slot before any read (see
// the SAFETY note), and the element types are `Copy` with no `Drop`. clippy cannot see that invariant.
#[allow(clippy::uninit_vec)]
fn materialise_grid(
    s: &VortexSession,
    chunks: &[StructArray],
    cols: &mut [ColumnData],
    threads: usize,
) -> Result<()> {
    let total: usize = chunks.iter().map(|c| c.len()).sum();

    // Build the disjoint write tasks. `split_at_mut` carves each column's output into per-row-group
    // sub-slices (proving disjointness to the borrow checker); each task owns its slice + the
    // still-encoded field it decompresses into it.
    let mut tasks: Vec<GridTask> = Vec::with_capacity(cols.len() * chunks.len());
    // One task per (column, row-group): pre-size the column, then carve it into per-row-group
    // sub-slices with `split_at_mut` (disjoint → no lock) and hand each slice + its still-encoded
    // field to a task that decompresses straight into place.
    macro_rules! push_grid {
        ($v:expr, $t:ty, $ci:expr) => {{
            let v: &mut Vec<$t> = $v;
            v.clear();
            v.reserve(total);
            // SAFETY: `$t` is a fixed-width numeric (`Copy`, no `Drop`). The disjoint per-row-group
            // tasks below cover every one of the `total` rows and each writes its slice exactly once
            // before any read, so exposing `total` uninitialised slots here — instead of a full
            // zero-fill of the output, which would waste the very memory bandwidth this path is
            // optimising — is sound. Caller invariant on error: if any task returns `Err`, this
            // function returns `Err` and the caller must discard `cols` without reading it (every
            // current caller does — `decode_inner` drops it via `?`); the elements are `Copy`/no-`Drop`,
            // so dropping a partially-written column never observes the uninitialised tail.
            unsafe {
                v.set_len(total);
            }
            let mut rest: &mut [$t] = v.as_mut_slice();
            for chunk in chunks.iter() {
                let (head, tail) = rest.split_at_mut(chunk.len());
                rest = tail;
                let field = chunk.unmasked_field($ci).clone();
                tasks.push(Box::new(move |ctx: &mut ExecutionCtx| -> Result<()> {
                    let p: PrimitiveArray = field.execute(ctx).map_err(ze)?;
                    head.copy_from_slice(p.as_slice::<$t>());
                    Ok(())
                }));
            }
        }};
    }
    for (ci, col) in cols.iter_mut().enumerate() {
        match col {
            ColumnData::I8(v) => push_grid!(v, i8, ci),
            ColumnData::I16(v) => push_grid!(v, i16, ci),
            ColumnData::I32(v) => push_grid!(v, i32, ci),
            ColumnData::I64(v) => push_grid!(v, i64, ci),
            ColumnData::U8(v) => push_grid!(v, u8, ci),
            ColumnData::U16(v) => push_grid!(v, u16, ci),
            ColumnData::U32(v) => push_grid!(v, u32, ci),
            ColumnData::U64(v) => push_grid!(v, u64, ci),
            ColumnData::F32(v) => push_grid!(v, f32, ci),
            ColumnData::F64(v) => push_grid!(v, f64, ci),
            _ => unreachable!("materialise_grid is numeric-only (guarded by is_grid_eligible)"),
        }
    }

    // Round-robin the tasks into one bucket per worker; each worker runs its bucket with its own
    // ExecutionCtx. Tasks are near-uniform (one row-group each), so round-robin balances well.
    let nb = threads.max(1).min(tasks.len().max(1));
    let mut buckets: Vec<Vec<GridTask>> = (0..nb).map(|_| Vec::new()).collect();
    for (k, t) in tasks.into_iter().enumerate() {
        buckets[k % nb].push(t);
    }
    std::thread::scope(|sc| -> Result<()> {
        let handles: Vec<_> = buckets
            .into_iter()
            .map(|bucket| {
                let s = s.clone(); // Arc-backed
                sc.spawn(move || -> Result<()> {
                    let mut ctx = s.create_execution_ctx();
                    for t in bucket {
                        t(&mut ctx)?;
                    }
                    Ok(())
                })
            })
            .collect();
        for h in handles {
            h.join()
                .map_err(|_| Error::Codec("table grid-decode worker panicked".into()))??;
        }
        Ok(())
    })
}

/// Drive the scan to completion, returning one canonical [`StructArray`] per row-group. The
/// struct is canonical but its *fields* are still encoded, so this is cheap (~2 % of a read) and
/// holds no more memory than the blob already resident in the caller's hands.
async fn scan_chunks(
    s: &VortexSession,
    blob: &[u8],
    projection: Option<&[&str]>,
) -> Result<Vec<StructArray>> {
    scan_chunks_at(s, blob, projection, None).await
}

/// [`scan_chunks`] with an optional **row selection** pushed into the scan.
///
/// `rows` must be sorted and unique; Vortex's `Selection::IncludeByIndex` is defined over sorted
/// indices and yields the selected rows in index order, not in the order they were requested.
/// Pushing the selection down means only the segments covering those rows are read and decoded —
/// as opposed to materialising the block and indexing it afterwards.
async fn scan_chunks_at(
    s: &VortexSession,
    blob: &[u8],
    projection: Option<&[&str]>,
    rows: Option<&[u64]>,
) -> Result<Vec<StructArray>> {
    let mut ctx = s.create_execution_ctx();
    let scan = s
        .open_options()
        .open_buffer(ByteBuffer::copy_from(blob))
        .map_err(ze)?
        .scan()
        .map_err(ze)?;
    let scan = match projection {
        Some(names) => scan.with_projection(select(names.to_vec(), root())),
        None => scan,
    };
    let scan = match rows {
        Some(idx) => scan.with_row_indices(Buffer::copy_from(idx)),
        None => scan,
    };
    let stream = scan.into_array_stream().map_err(ze)?;
    futures::pin_mut!(stream);
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk.map_err(ze)?.execute(&mut ctx).map_err(ze)?);
    }
    Ok(chunks)
}

/// Decode the whole table from a block payload (inverse of [`encode`]).
///
/// Sizes its own fan-out against the whole machine. A caller that is *already* decoding many
/// blocks in parallel should use [`decode_with_workers`] to pin each block to one worker instead
/// of nesting two fan-outs.
pub fn decode(spec: &TableSpec, blob: &[u8]) -> Result<TableData> {
    decode_inner(spec, blob, None)
}

/// [`decode`] with the materialise fan-out pinned to `workers` threads (`1` = fully serial).
///
/// The point is composition. `decode` sizes itself to `available_parallelism()`, which is right
/// for one decode on an idle machine and wrong inside a caller's own parallel loop — N concurrent
/// `decode`s would each spawn N workers. Decoding a cohort of blocks across a thread pool should
/// therefore pass `1` here and keep the parallelism at the outer level, where it already has
/// better work granularity.
pub fn decode_with_workers(spec: &TableSpec, blob: &[u8], workers: usize) -> Result<TableData> {
    decode_inner(spec, blob, Some(workers))
}

fn decode_inner(spec: &TableSpec, blob: &[u8], workers: Option<usize>) -> Result<TableData> {
    // Accumulators, one per declared column (column order == struct field order on write),
    // pre-sized to the declared row count — see [`empty_column_with_capacity`], this is the
    // read's dominant cost.
    let rows = spec.rows as usize;
    let mut cols: Vec<ColumnData> = spec
        .columns
        .iter()
        .map(|c| empty_column_for(c, rows))
        .collect::<Result<_>>()?;

    with_read_session(|rt, s| {
        let chunks = rt.block_on(scan_chunks(s, blob, None))?;
        match workers {
            Some(n) => materialise_with(s, &chunks, &mut cols, n),
            None => materialise(s, &chunks, &mut cols),
        }
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
    let mut out = empty_column_for(col, spec.rows as usize)?;
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
            extend_field(&mut out, st.unmasked_field(0).clone(), &mut ctx)?;
        }
        Ok::<(), Error>(())
    })?;
    Ok(out)
}

/// Decode a **projected subset** of columns in a **single session** — Vortex
/// scans only the named columns' layout segments (projection pushdown) and pays
/// the session/encoding setup **once**, unlike N separate [`decode_column`]
/// calls. The result columns are in `names` order.
///
/// This is the read shape the replay pipeline wants (a few columns of a wide
/// listmode), and where Vortex's columnar layout beats a full [`decode`].
pub fn decode_projected(spec: &TableSpec, blob: &[u8], names: &[&str]) -> Result<TableData> {
    let projected: Vec<&Column> = names
        .iter()
        .map(|&n| {
            spec.columns
                .iter()
                .find(|c| c.name == n)
                .ok_or_else(|| Error::Codec(format!("table has no column '{n}'")))
        })
        .collect::<Result<_>>()?;
    let mut cols: Vec<ColumnData> = projected
        .iter()
        .map(|c| empty_column_for(c, spec.rows as usize))
        .collect::<Result<_>>()?;
    with_read_session(|rt, s| {
        let chunks = rt.block_on(scan_chunks(s, blob, Some(names)))?;
        materialise(s, &chunks, &mut cols)
    })?;
    Ok(names.iter().map(|n| n.to_string()).zip(cols).collect())
}

/// Build the `{hash, stats}` chunk-index (ADR-0028 §3) for a table block, splitting on the **same**
/// fixed [`ROWS_PER_GROUP`] row-groups [`encode`] uses. Each entry carries the row-group's content digest
/// (BLAKE3 over the group's little-endian column bytes — recomputable from the decoded group, independent
/// of the Vortex byte layout) and the chunk statistics of `stat_column` (which must be an integer column,
/// see [`ColumnData::as_i64`]). Other columns still feed each group's digest; only the stats come from
/// `stat_column`. `index.root()` is the block's sub-block Merkle root (ADR-0028 §1), and `index.prune()`
/// skips row-groups a ranged read can't hit. Per #221-B, the *index* leaf may later be finer than
/// `ROWS_PER_GROUP`; this first wiring uses the encoder's row-groups 1:1.
///
/// ```
/// use tessera_core::block::table::{Column, TableSpec};
/// use tessera_io::table::{table_chunk_index, ColumnData, TableData};
///
/// let spec = TableSpec {
///     columns: vec![Column { name: "t".into(), dtype: "u8".into(), codec: None, ..Default::default() }],
///     rows: 3,
///     row_index: None,
/// };
/// let data: TableData = vec![("t".into(), ColumnData::U64(vec![10, 20, 30]))];
/// let idx = table_chunk_index(&spec, &data, "t").unwrap();
///
/// assert_eq!(idx.len(), 1); // 3 rows <= ROWS_PER_GROUP -> a single row-group
/// assert_eq!(idx.aggregate().max, Some(30)); // stats over the column
/// assert_eq!(idx.prune(0, 15), vec![0]); // the group spans [10, 30] -> overlaps [0, 15]
/// assert!(idx.root().starts_with("blake3:")); // sub-block MMR root
/// ```
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

/// A committed block: its manifest reference paired with its payload bytes.
type EncodedBlock = (BlockRef, BlockPayload);

/// Fused table block emit (ADR-0028 §5): encode the table block **and** build its `{hash, stats}`
/// chunk-index sidecar over `stat_column` in one call — the table counterpart to
/// [`crate::array::array_block_with_index`]. The sidecar is built only when `stat_column` names an
/// **integer** column present in the data (else `None`: the row-group digests still roll up through the
/// data block, but there are no prunable stats). Real encode/index errors propagate.
pub fn table_block_with_index(
    name: &str,
    spec: &TableSpec,
    data: &TableData,
    stat_column: Option<&str>,
) -> Result<(EncodedBlock, Option<EncodedBlock>)> {
    let block = table_block(name, spec, data)?;
    let sidecar = match stat_column {
        Some(col) if data.iter().any(|(n, c)| n == col && c.as_i64().is_some()) => {
            let index = table_chunk_index(spec, data, col)?;
            Some(crate::chunk_index::chunk_index_block(name, &index)?)
        }
        _ => None, // no integer stat column → no prunable-stats sidecar (ADR-0028 §3 integer core)
    };
    Ok((block, sidecar))
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
            ..Default::default()
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

    /// The column-parallel materialise must be **bit-identical** to the serial one, for every
    /// dtype (including the `Bool`/`Utf8` columns that take their own decode path) and across
    /// several row-groups — worker `t` owns columns `[t*per, (t+1)*per)` and walks the row-groups
    /// in order, so any ordering or off-by-one in the split shows up here.
    /// Nullable columns survive a full encode → decode round-trip with the mask intact, through
    /// every public read entry point (whole-table, projected, single-column).
    #[test]
    fn nullable_column_round_trips_through_every_read_path() {
        let rows = 1000usize;
        fn present(k: usize, rows: usize) -> bool {
            k >= 3 && !k.is_multiple_of(7) && k != rows - 1
        }
        let spec = TableSpec {
            columns: vec![col("t", "u8"), col("e", "i2").nullable()],
            rows: rows as u64,
            row_index: Some("t".into()),
        };
        let data: TableData = vec![
            ("t".into(), ColumnData::U64((0..rows as u64).collect())),
            (
                "e".into(),
                ColumnData::Nullable {
                    // Canonical form: 0 under every null. Encoding normalises masked slots to the
                    // dtype default (see the `Nullable` docs), so only canonical input compares
                    // equal after a round-trip.
                    values: Box::new(ColumnData::I16(
                        (0..rows)
                            .map(|k| {
                                if present(k, rows) {
                                    (k % 511) as i16
                                } else {
                                    0
                                }
                            })
                            .collect(),
                    )),
                    // a run of leading nulls, a trailing null, and scattered ones
                    validity: (0..rows).map(|k| present(k, rows)).collect(),
                },
            ),
        ];
        let blob = encode(&spec, &data).unwrap();
        assert_eq!(decode(&spec, &blob).unwrap(), data, "whole-table decode");
        assert_eq!(
            decode_projected(&spec, &blob, &["e"]).unwrap(),
            vec![data[1].clone()],
            "projected decode"
        );
        assert_eq!(
            decode_column(&spec, &blob, "e").unwrap(),
            data[1].1,
            "single-column decode"
        );
        // Deterministic, like every other encode path.
        assert_eq!(
            encode(&spec, &data).unwrap(),
            blob,
            "nullable encode not deterministic"
        );
    }

    /// An all-null and an all-valid column both round-trip — Vortex collapses a constant validity
    /// mask internally, so these take a different path from a mixed mask.
    #[test]
    fn nullable_all_null_and_all_valid_round_trip() {
        for validity_of in [0usize, 1] {
            let rows = 128usize;
            let spec = TableSpec {
                columns: vec![col("v", "f8").nullable()],
                rows: rows as u64,
                row_index: None,
            };
            let data: TableData = vec![(
                "v".into(),
                ColumnData::Nullable {
                    values: Box::new(ColumnData::F64(
                        (0..rows)
                            .map(|k| {
                                if validity_of == 1 {
                                    k as f64 * 0.5
                                } else {
                                    0.0
                                }
                            })
                            .collect(),
                    )),
                    validity: vec![validity_of == 1; rows],
                },
            )];
            let blob = encode(&spec, &data).unwrap();
            assert_eq!(
                decode(&spec, &blob).unwrap(),
                data,
                "constant-validity mask ({validity_of}) did not round-trip"
            );
        }
    }

    /// The corpus-stability gate: a NON-nullable column must encode to exactly the bytes it did
    /// before #330 existed. If this drifts, every committed fixture's `content_hash` changes and
    /// the whole conformance corpus needs regeneration.
    #[test]
    fn non_nullable_encode_is_unchanged_by_the_nullable_feature() {
        let rows = 4096usize;
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
        // Round-trips, carries no validity, and the spec column serialises with no `nullable` key
        // (asserted in tessera-core) — together these pin the golden bytes.
        let back = decode(&spec, &blob).unwrap();
        assert_eq!(back, data);
        assert!(
            !matches!(back[1].1, ColumnData::Nullable { .. }),
            "a non-nullable spec column must not decode as Nullable"
        );
    }

    /// `validate` is the single gate: spec/data nullability must agree, the mask must cover the
    /// values, nesting is rejected, and the unsupported dtypes are named rather than panicking.
    #[test]
    fn validate_rejects_malformed_nullable_columns() {
        let nullable_spec = |dtype: &str| TableSpec {
            columns: vec![col("v", dtype).nullable()],
            rows: 3,
            row_index: None,
        };
        let bare_spec = TableSpec {
            columns: vec![col("v", "i2")],
            rows: 3,
            row_index: None,
        };
        let nullable = |values: ColumnData, validity: Vec<bool>| ColumnData::Nullable {
            values: Box::new(values),
            validity,
        };

        // spec says nullable, data is bare
        let e = encode(
            &nullable_spec("i2"),
            &vec![("v".into(), ColumnData::I16(vec![1, 2, 3]))],
        )
        .unwrap_err();
        assert!(format!("{e}").contains("no validity mask"), "got: {e}");

        // data is nullable, spec is not
        let e = encode(
            &bare_spec,
            &vec![(
                "v".into(),
                nullable(ColumnData::I16(vec![1, 2, 3]), vec![true; 3]),
            )],
        )
        .unwrap_err();
        assert!(format!("{e}").contains("not marked nullable"), "got: {e}");

        // mask shorter than the values
        let e = encode(
            &nullable_spec("i2"),
            &vec![(
                "v".into(),
                nullable(ColumnData::I16(vec![1, 2, 3]), vec![true; 2]),
            )],
        )
        .unwrap_err();
        assert!(format!("{e}").contains("validity flags"), "got: {e}");

        // nested nullable
        let e = encode(
            &nullable_spec("i2"),
            &vec![(
                "v".into(),
                nullable(
                    nullable(ColumnData::I16(vec![1, 2, 3]), vec![true; 3]),
                    vec![true; 3],
                ),
            )],
        )
        .unwrap_err();
        assert!(format!("{e}").contains("nested"), "got: {e}");

        // out-of-scope dtypes are REJECTED, not panicked — nullability is numeric-only today
        for (dtype, values) in [
            ("b1", ColumnData::Bool(vec![true, false, true])),
            (
                "str",
                ColumnData::Utf8(vec!["a".into(), "b".into(), "c".into()]),
            ),
        ] {
            let e = encode(
                &nullable_spec(dtype),
                &vec![("v".into(), nullable(values, vec![true; 3]))],
            )
            .unwrap_err();
            assert!(
                format!("{e}").contains("not supported yet"),
                "nullable {dtype} should be a typed rejection, got: {e}"
            );
        }
    }

    /// Two columns that differ ONLY beneath their nulls must encode to identical bytes.
    ///
    /// This is what makes `content_hash` well-defined for nullable data: a producer must not be
    /// able to change a sealed product's identity by varying whatever sits in a masked-out slot.
    #[test]
    fn bytes_are_independent_of_values_under_nulls() {
        let rows = 512usize;
        let spec = TableSpec {
            columns: vec![col("v", "i4").nullable()],
            rows: rows as u64,
            row_index: None,
        };
        let validity: Vec<bool> = (0..rows).map(|k| k % 3 != 0).collect();
        let build = |filler: i32| -> TableData {
            vec![(
                "v".into(),
                ColumnData::Nullable {
                    values: Box::new(ColumnData::I32(
                        (0..rows)
                            .map(|k| if validity[k] { k as i32 } else { filler })
                            .collect(),
                    )),
                    validity: validity.clone(),
                },
            )]
        };
        let a = encode(&spec, &build(0)).unwrap();
        let b = encode(&spec, &build(i32::MIN)).unwrap();
        let c = encode(&spec, &build(999_999)).unwrap();
        assert_eq!(a, b, "bytes changed with the filler under nulls");
        assert_eq!(a, c, "bytes changed with the filler under nulls");
        // ...and the decode is the canonical form regardless of which one was written.
        assert_eq!(decode(&spec, &c).unwrap(), build(0));
    }

    /// Row-index pushdown must be BIT-EXACT with decoding the block and slicing the same rows.
    ///
    /// This is the correctness contract for `decode_rows`: pushing the selection into Vortex reads
    /// different segments than a full scan, so the two paths could diverge (wrong rows, wrong
    /// order, or a chunk-boundary off-by-one) without anything else failing.
    #[test]
    fn decode_rows_matches_decode_then_slice() {
        let rows = ROWS_PER_GROUP + 3000; // spans a row-group boundary
        let (spec, data) = all_dtype_table(rows);
        let blob = encode(&spec, &data).unwrap();
        let full = decode(&spec, &blob).unwrap();

        let cases: Vec<Vec<u64>> = vec![
            vec![0],
            vec![(rows - 1) as u64],
            vec![0, 1, 2, 3],
            // straddling the row-group boundary
            vec![
                (ROWS_PER_GROUP - 2) as u64,
                (ROWS_PER_GROUP - 1) as u64,
                ROWS_PER_GROUP as u64,
                (ROWS_PER_GROUP + 1) as u64,
            ],
            // scattered across the whole table
            (0..rows as u64).step_by(1013).collect(),
            vec![7, 9000, (rows - 1) as u64],
        ];
        for want in cases {
            let got = decode_rows(&spec, &blob, &want).unwrap();
            let expected: TableData = full
                .iter()
                .map(|(n, c)| {
                    let mut acc = ColumnData::from_le_bytes(c.numpy_code(), &[]).unwrap();
                    for &r in &want {
                        acc.extend(&c.slice(r as usize, r as usize + 1)).unwrap();
                    }
                    (n.clone(), acc)
                })
                .collect();
            assert_eq!(got, expected, "decode_rows diverged for {want:?}");
        }
        // Empty selection is a well-defined empty table, not an error.
        let empty = decode_rows(&spec, &blob, &[]).unwrap();
        assert!(empty.iter().all(|(_, c)| c.is_empty()));
    }

    /// Unsorted, duplicated, or out-of-range indices are typed errors — `Selection::IncludeByIndex`
    /// is only defined over sorted-unique indices, so accepting them would silently return the
    /// wrong rows.
    #[test]
    fn decode_rows_rejects_malformed_selections() {
        let (spec, data) = all_dtype_table(64);
        let blob = encode(&spec, &data).unwrap();
        for bad in [vec![3u64, 1], vec![2, 2], vec![0, 64]] {
            let e = decode_rows(&spec, &blob, &bad).unwrap_err();
            let msg = format!("{e}");
            assert!(
                msg.contains("sorted and unique") || msg.contains("out of range"),
                "expected a typed rejection for {bad:?}, got: {msg}"
            );
        }
    }

    /// Chunk statistics must describe the values that are PRESENT: a NULL is skipped, not folded
    /// in as 0, or `min`/`max` pruning would exclude chunks that really do contain matches.
    #[test]
    fn as_i64_skips_nulls() {
        let c = ColumnData::Nullable {
            values: Box::new(ColumnData::I32(vec![5, -3, 100, 7])),
            validity: vec![true, false, true, false],
        };
        assert_eq!(c.as_i64(), Some(vec![5, 100]));
    }

    #[test]
    fn materialise_is_bit_identical_across_thread_counts() {
        let rows = ROWS_PER_GROUP + 4242; // 2 row-groups + remainder
        let (mut spec, mut data) = all_dtype_table(rows);
        data.push((
            "b1".into(),
            ColumnData::Bool((0..rows).map(|k| k % 3 == 0).collect()),
        ));
        data.push((
            "str".into(),
            ColumnData::Utf8((0..rows).map(|k| format!("crystal-{}", k % 97)).collect()),
        ));
        // A nullable column too (#330): validity is assembled per worker in `materialise_with`,
        // so it is exactly the kind of state that can interleave wrongly under parallelism.
        data.push((
            "maybe".into(),
            ColumnData::Nullable {
                values: Box::new(ColumnData::I32(
                    (0..rows)
                        .map(|k| if k % 5 != 0 { k as i32 * 7 } else { 0 })
                        .collect(),
                )),
                validity: (0..rows).map(|k| k % 5 != 0).collect(),
            },
        ));
        spec.columns = data
            .iter()
            .map(|(n, c)| {
                let base = col(n, c.numpy_code());
                if matches!(c, ColumnData::Nullable { .. }) {
                    base.nullable()
                } else {
                    base
                }
            })
            .collect();
        let blob = encode(&spec, &data).unwrap();

        // `repeat` duplicates the scanned chunks, exercising the multi-chunk append loop
        // regardless of how the reader chooses to batch this particular file.
        let decoded = |threads: usize, repeat: usize| -> TableData {
            let mut cols: Vec<ColumnData> = spec
                .columns
                .iter()
                .map(|c| empty_column_for(c, rows * repeat))
                .collect::<Result<_>>()
                .unwrap();
            with_read_session(|rt, s| {
                let scanned = rt.block_on(scan_chunks(s, &blob, None)).unwrap();
                let chunks: Vec<StructArray> =
                    std::iter::repeat_n(scanned, repeat).flatten().collect();
                materialise_with(s, &chunks, &mut cols, threads).unwrap();
            });
            spec.columns
                .iter()
                .map(|c| c.name.clone())
                .zip(cols)
                .collect()
        };

        let serial = decoded(1, 1);
        assert_eq!(serial, data, "serial materialise must round-trip");
        // Every column must land in the same worker split regardless of worker count — including
        // more workers than columns (the empty-group edge).
        for threads in [2, 3, 4, 8, 64] {
            assert_eq!(
                decoded(threads, 1),
                serial,
                "materialise with {threads} workers diverged from serial"
            );
            assert_eq!(
                decoded(threads, 3),
                decoded(1, 3),
                "multi-chunk append order diverged at {threads} workers"
            );
        }
        // And both public entry points agree — the self-sizing one and the pinned one.
        assert_eq!(decode(&spec, &blob).unwrap(), data);
        for workers in [1, 2, 7] {
            assert_eq!(
                decode_with_workers(&spec, &blob, workers).unwrap(),
                data,
                "decode_with_workers({workers}) diverged"
            );
        }
    }

    #[test]
    fn table_block_with_index_emits_block_plus_optional_sidecar() {
        // ADR-0028 §5 fused emit (table counterpart): data block + optional chunk-index sidecar.
        let (spec, data) = all_dtype_table(10);
        // an integer stat column → block + sidecar; the sidecar == the separate composition.
        let ((blk, _), sidecar) = table_block_with_index("t", &spec, &data, Some("i8")).unwrap();
        let (blk_ref, _) = table_block("t", &spec, &data).unwrap();
        assert_eq!(blk.digest, blk_ref.digest);
        let (scar, _) = sidecar.expect("integer stat column yields a sidecar");
        let idx = table_chunk_index(&spec, &data, "i8").unwrap();
        let (expect, _) = crate::chunk_index::chunk_index_block("t", &idx).unwrap();
        assert_eq!(scar.digest, expect.digest);
        assert_eq!(scar.name, expect.name);
        // None, a float column, or an absent column → no prunable-stats sidecar.
        assert!(table_block_with_index("t", &spec, &data, None)
            .unwrap()
            .1
            .is_none());
        assert!(table_block_with_index("t", &spec, &data, Some("f8"))
            .unwrap()
            .1
            .is_none());
        assert!(table_block_with_index("t", &spec, &data, Some("nope"))
            .unwrap()
            .1
            .is_none());
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

    /// A short-lived runtime must not poison the long-lived pooled read session.
    ///
    /// `encode`/`encode_streaming`/`decode_column` each build their own
    /// `CurrentThreadRuntime` and drop it on return, while `decode`/`decode_projected`
    /// use the per-thread pooled [`READ_RT`]. When every session was cloned from one
    /// cached template, `clone().with_handle(..)` rebound `Arc`-shared state, so the
    /// short-lived runtime's death left the pooled session dangling and this third
    /// call panicked with `Attempted to use a Handle after its runtime was dropped`.
    ///
    /// The ordering is the whole test: it only reproduces when a pooled read, a
    /// short-lived-runtime read, and another pooled read share **one process** — which
    /// is why nextest (a process per test) could never surface it, and only the
    /// `tessera-py-import` check did.
    #[test]
    fn short_lived_runtime_does_not_poison_pooled_session() {
        use tessera_core::block::table::{Column, TableSpec};
        let spec = TableSpec {
            columns: vec![Column::new("idx", "u4"), Column::new("en", "f4")],
            rows: 5,
            row_index: None,
        };
        let data: TableData = vec![
            ("idx".into(), ColumnData::U32((0..5u32).collect())),
            ("en".into(), ColumnData::F32(vec![0.5, 1.5, 2.5, 3.5, 4.5])),
        ];
        let blob = encode(&spec, &data).unwrap();

        assert_eq!(decode(&spec, &blob).unwrap(), data, "pooled decode");
        // Builds and drops its own runtime — the poisoning step.
        decode_column(&spec, &blob, "idx").unwrap();
        assert_eq!(
            decode(&spec, &blob).unwrap(),
            data,
            "pooled session poisoned by a dropped short-lived runtime"
        );
    }

    #[test]
    fn bool_and_utf8_columns_roundtrip() {
        use tessera_core::block::table::{Column, TableSpec};
        let n = 300usize;
        let flags: Vec<bool> = (0..n).map(|k| k % 3 == 0).collect();
        let origins: Vec<String> = (0..n)
            .map(|k| ["annih511", "prompt_nuclear", "other"][k % 3].to_string())
            .collect();
        let data: TableData = vec![
            ("flag".into(), ColumnData::Bool(flags.clone())),
            ("origin".into(), ColumnData::Utf8(origins.clone())),
        ];
        let spec = TableSpec {
            columns: vec![Column::new("flag", "b1"), Column::new("origin", "str")],
            rows: n as u64,
            row_index: None,
        };
        // Vortex round-trip (bit-packed bool + FSST/dict-chosen strings).
        let blob = encode(&spec, &data).unwrap();
        assert_eq!(
            decode(&spec, &blob).unwrap(),
            data,
            "bool/utf8 vortex roundtrip"
        );
        assert_eq!(
            encode(&spec, &data).unwrap(),
            blob,
            "bool/utf8 non-deterministic"
        );
        // The low-cardinality origin column compresses: encoding the *same shape* with unique
        // strings instead is materially bigger, i.e. the compressor exploited the repeats
        // (dict/FSST). Comparing against the raw string bytes would be unfair — a Vortex file
        // carries a fixed footer/metadata cost that dwarfs 300 short strings.
        let unique: Vec<String> = (0..n).map(|k| format!("origin-{k:07}")).collect();
        let hi_card: TableData = vec![
            ("flag".into(), ColumnData::Bool(flags.clone())),
            ("origin".into(), ColumnData::Utf8(unique)),
        ];
        let hi_blob = encode(&spec, &hi_card).unwrap();
        assert!(
            blob.len() < hi_blob.len(),
            "expected the repeated-string column to compress below the unique-string one: \
             {} vs {}",
            blob.len(),
            hi_blob.len()
        );
        // LE-bytes round-trip (cross-runtime path).
        let b = ColumnData::Bool(flags.clone());
        assert_eq!(
            ColumnData::from_le_bytes("b1", &b.to_le_bytes()).unwrap(),
            b
        );
        let u = ColumnData::Utf8(origins);
        assert_eq!(
            ColumnData::from_le_bytes("str", &u.to_le_bytes()).unwrap(),
            u
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
    fn f64_column_is_compressed_by_pco_not_stored_raw() {
        // Regression guard for #380. A high-cardinality continuous f64 column (a physics-like energy
        // grid spanning many orders of magnitude — the shape that surfaced the bug) must be
        // compressed by the registered Pco scheme, not fall through to flat/raw. Before Pco was
        // registered, excluding ALP left floats with NO applicable scheme and the block was
        // byte-for-byte the raw payload (measured +0.06%). Also confirms exact round-trip.
        const N: usize = 200_000;
        let (lo, hi) = (1e-5f64, 2e8f64); // 13 orders of magnitude, monotone increasing
        let energy: Vec<f64> = (0..N)
            .map(|i| lo * (hi / lo).powf(i as f64 / (N as f64 - 1.0)))
            .collect();
        let data: TableData = vec![("energy_ev".into(), ColumnData::F64(energy.clone()))];
        let spec = TableSpec {
            columns: vec![col("energy_ev", "f8")],
            rows: N as u64,
            row_index: None,
        };
        let encoded = encode(&spec, &data).unwrap();
        let raw = N * std::mem::size_of::<f64>();
        // The raw fall-through was ~1.0006x; Pco compresses a smooth monotone column far below that
        // (measured ~0.12x). Bound kept comfortably above the observed ratio so it guards the
        // regression without being brittle to Pco-version drift.
        assert!(
            encoded.len() < raw * 7 / 10,
            "f64 column not compressed — Pco unregistered? (#380): {} of {raw} raw bytes ({:.3}x)",
            encoded.len(),
            encoded.len() as f64 / raw as f64
        );
        // Exact round-trip still holds.
        let back = decode(&spec, &encoded).unwrap();
        let ColumnData::F64(g) = &back[0].1 else {
            panic!("expected F64 column back")
        };
        assert_eq!(g, &energy);
    }

    #[test]
    fn grid_decode_numeric_matches_serial_across_row_groups() {
        // The all-numeric GRID decode path (materialise_grid, via decode's auto routing) must be
        // bit-identical to the serial column path. Size the table above PARALLEL_MATERIALISE_MIN_VALUES
        // (rows × cols > 2^20) and across several row-groups so the grid actually fans out over the
        // (column × row-group) grid, then cross-check the auto path (grid) against a pinned
        // single-worker column decode.
        let rows = ROWS_PER_GROUP * 7 + 321; // 8 row-groups incl. a partial tail; ×3 cols > 2^20
        let mut lcg = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            lcg = lcg
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            lcg
        };
        let a: Vec<f64> = (0..rows).map(|_| (next() >> 12) as f64 * 1e-9).collect();
        let data: TableData = vec![
            ("a".into(), ColumnData::F64(a)),
            (
                "b".into(),
                ColumnData::I64((0..rows as i64).map(|k| k * 7 - 3).collect()),
            ),
            (
                "c".into(),
                ColumnData::U32(
                    (0..rows as u32)
                        .map(|k| k.wrapping_mul(2654435761))
                        .collect(),
                ),
            ),
        ];
        let spec = TableSpec {
            columns: vec![col("a", "f8"), col("b", "i8"), col("c", "u4")],
            rows: rows as u64,
            row_index: None,
        };
        let blob = encode(&spec, &data).unwrap();
        let grid = decode(&spec, &blob).unwrap(); // auto → grid (all numeric, over threshold)
        let serial = decode_with_workers(&spec, &blob, 1).unwrap(); // pinned single-worker column path
        assert_eq!(grid, data, "grid decode diverged from the input");
        assert_eq!(
            grid, serial,
            "grid decode diverged from serial column decode"
        );
    }

    #[test]
    fn grid_decode_covers_every_numeric_dtype_and_edges() {
        // Exercise EVERY `push_grid!` arm — all 10 numeric dtypes — through the grid, at two edge
        // shapes: a partial-tail row-group and an exact multiple of ROWS_PER_GROUP. Values use
        // truncating `as` / `wrapping_mul` so there is no debug-mode overflow. Grid (auto) must be
        // bit-identical to the input and to the serial column path, and must actually take the grid.
        for rows in [ROWS_PER_GROUP * 2 + 777, ROWS_PER_GROUP * 3] {
            let data: TableData = vec![
                (
                    "i1".into(),
                    ColumnData::I8((0..rows).map(|k| k as i8).collect()),
                ),
                (
                    "i2".into(),
                    ColumnData::I16((0..rows).map(|k| (k as i16).wrapping_mul(7)).collect()),
                ),
                (
                    "i4".into(),
                    ColumnData::I32((0..rows).map(|k| k as i32 - 5).collect()),
                ),
                (
                    "i8".into(),
                    ColumnData::I64((0..rows).map(|k| (k as i64).wrapping_mul(11)).collect()),
                ),
                (
                    "u1".into(),
                    ColumnData::U8((0..rows).map(|k| k as u8).collect()),
                ),
                (
                    "u2".into(),
                    ColumnData::U16((0..rows).map(|k| k as u16).collect()),
                ),
                (
                    "u4".into(),
                    ColumnData::U32(
                        (0..rows)
                            .map(|k| (k as u32).wrapping_mul(2654435761))
                            .collect(),
                    ),
                ),
                (
                    "u8".into(),
                    ColumnData::U64(
                        (0..rows)
                            .map(|k| (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
                            .collect(),
                    ),
                ),
                (
                    "f4".into(),
                    ColumnData::F32((0..rows).map(|k| k as f32 * 0.5).collect()),
                ),
                (
                    "f8".into(),
                    ColumnData::F64((0..rows).map(|k| k as f64 * 1e-6).collect()),
                ),
            ];
            let spec = TableSpec {
                columns: data.iter().map(|(n, c)| col(n, c.numpy_code())).collect(),
                rows: rows as u64,
                row_index: None,
            };
            let blob = encode(&spec, &data).unwrap();
            GRID_DECODES.with(|c| c.set(0));
            let grid = decode(&spec, &blob).unwrap();
            assert!(
                GRID_DECODES.with(|c| c.get()) >= 1,
                "rows={rows}: decode did not take the grid path"
            );
            let serial = decode_with_workers(&spec, &blob, 1).unwrap();
            assert_eq!(grid, data, "rows={rows}: grid decode != input");
            assert_eq!(
                grid, serial,
                "rows={rows}: grid decode != serial column decode"
            );
        }
    }

    #[test]
    fn grid_routing_is_gated_by_size_and_dtype() {
        // Non-flaky drift guard on the ROUTING predicate (thread-local counter, no timing): a large
        // all-numeric table must take the grid; a small one (below PARALLEL_MATERIALISE_MIN_VALUES)
        // and a mixed-type one (a Bool present → not grid-eligible) must NOT.
        let numeric = |rows: usize| -> (TableSpec, TableData) {
            let data: TableData = vec![
                (
                    "a".into(),
                    ColumnData::F64((0..rows).map(|k| k as f64).collect()),
                ),
                ("b".into(), ColumnData::I64((0..rows as i64).collect())),
                ("c".into(), ColumnData::U32((0..rows as u32).collect())),
            ];
            let spec = TableSpec {
                columns: data.iter().map(|(n, c)| col(n, c.numpy_code())).collect(),
                rows: rows as u64,
                row_index: None,
            };
            (spec, data)
        };
        let took_grid = |spec: &TableSpec, blob: &[u8]| -> usize {
            GRID_DECODES.with(|c| c.set(0));
            let _ = decode(spec, blob).unwrap();
            GRID_DECODES.with(|c| c.get())
        };

        let big = ROWS_PER_GROUP * 6; // ×3 cols > 2^20 threshold
        let (spec_b, data_b) = numeric(big);
        let blob_b = encode(&spec_b, &data_b).unwrap();
        assert!(
            took_grid(&spec_b, &blob_b) >= 1,
            "large numeric table must take the grid"
        );

        let (spec_s, data_s) = numeric(128);
        let blob_s = encode(&spec_s, &data_s).unwrap();
        assert_eq!(
            took_grid(&spec_s, &blob_s),
            0,
            "small table must stay serial, not grid"
        );

        let mut data_m = data_b.clone();
        data_m.push((
            "flag".into(),
            ColumnData::Bool((0..big).map(|k| k % 2 == 0).collect()),
        ));
        let spec_m = TableSpec {
            columns: data_m.iter().map(|(n, c)| col(n, c.numpy_code())).collect(),
            rows: big as u64,
            row_index: None,
        };
        let blob_m = encode(&spec_m, &data_m).unwrap();
        assert_eq!(
            took_grid(&spec_m, &blob_m),
            0,
            "mixed-type (Bool) table must use the column fallback, not the grid"
        );
    }

    #[test]
    fn grid_decode_perf_ratchet_narrow_table() {
        // Coarse performance drift floor (#352): the grid must actually parallelise a NARROW table —
        // i.e. it must NOT collapse back to serial speed. Relative (machine-independent): the auto
        // (grid) decode of a 2-column table must beat the 1-worker column-serial decode by a generous
        // margin, best-of-N. Gated at >=8 cores on purpose: it runs on dev machines with headroom
        // (a real perf guard) and is SKIPPED on the small, build-saturated CI runners where a timing
        // assertion would be flaky. The non-flaky CI guard is the *routing* test
        // (`grid_routing_is_gated_by_size_and_dtype`); absolute-throughput tracking belongs in a
        // dedicated perf job, not a unit gate.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        if cores < 8 {
            return;
        }
        let rows = 4_000_000usize;
        let data: TableData = vec![
            (
                "a".into(),
                ColumnData::F64((0..rows).map(|k| k as f64 * 1e-6).collect()),
            ),
            ("b".into(), ColumnData::I64((0..rows as i64).collect())),
        ];
        let spec = TableSpec {
            columns: data.iter().map(|(n, c)| col(n, c.numpy_code())).collect(),
            rows: rows as u64,
            row_index: None,
        };
        let blob = encode(&spec, &data).unwrap();
        let best = |run: &dyn Fn()| -> f64 {
            let mut m = f64::MAX;
            for _ in 0..7 {
                let t = std::time::Instant::now();
                run();
                m = m.min(t.elapsed().as_secs_f64());
            }
            m
        };
        let _ = decode(&spec, &blob).unwrap(); // warm
        let grid = best(&|| {
            decode(&spec, &blob).unwrap();
        });
        let serial = best(&|| {
            decode_with_workers(&spec, &blob, 1).unwrap();
        });
        let speedup = serial / grid;
        assert!(
            speedup > 1.2,
            "grid decode not parallelising a narrow table: {speedup:.2}× vs 1-worker serial \
             (regression — grid path disabled, serialised, or the zero-fill returned?)"
        );
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
    fn partition_blocks_and_block_name_are_the_format_partition_ssot() {
        // The partition + naming logic is the SSoT both whole-file and streamed ingest call into
        // (so they cannot disagree on the per-block split / content_hash). Test the partition logic
        // at a small block size — independent of the production BLOCK_ROWS constant — and check the
        // small-stays-single naming invariant + the multi-block naming.

        // ── partition logic (worker/RAM-independent: depends ONLY on rows + block size)
        assert_eq!(partition_blocks(0, 4), 1, "empty → one (empty) block");
        assert_eq!(partition_blocks(1, 4), 1, "below the ceiling → one block");
        assert_eq!(partition_blocks(4, 4), 1, "exact == one block, no extra");
        assert_eq!(partition_blocks(5, 4), 2, "just-over → 2 blocks");
        assert_eq!(partition_blocks(8, 4), 2, "exact 2x → 2 blocks");
        assert_eq!(partition_blocks(9, 4), 3, "rolls over by one → 3 blocks");
        // production helper agrees with the explicit form at the real BLOCK_ROWS.
        assert_eq!(block_count(BLOCK_ROWS as u64), 1);
        assert_eq!(block_count((BLOCK_ROWS as u64) + 1), 2);

        // ── naming: small-stays-single (no NNNN suffix), multi-block uses zero-padded 4-digit suffix
        assert_eq!(
            block_name("events", 0, 0),
            "events",
            "empty product → bare name"
        );
        assert_eq!(
            block_name("events", 0, 1),
            "events",
            "single block → bare name"
        );
        assert_eq!(block_name("events", 0, 2), "events_0000");
        assert_eq!(block_name("events", 7, 8), "events_0007");
        assert_eq!(block_name("events", 1234, 9999), "events_1234");
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
