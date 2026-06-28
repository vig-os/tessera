//! GE-HDF5 vendor-raw ingest (#208) — read GE Discovery MI Gen2 listmode coincidence events
//! (an HDF5 **compound** dataset, row-major) into a Tessera `listmode` product, **transposing to
//! columnar** on the way in (the fd5 #193 fix: a single-column projection on a row-major compound
//! costs a full-table read; Vortex columns don't). Lossless: native dtypes preserved.
//!
//! **Generic compound reader (#222).** [`read_compound`] / [`stream_compound`] handle ANY HDF5
//! compound dataset by introspecting the on-disk datatype descriptor — there are no per-record
//! structs (the old hardcoded `Rec2p`/`Rec3p` + per-record decoders are gone). Per-field scalar and
//! fixed-array members fan out to flat columns via the same naming rule the old hardcoded readers
//! used (`name` → `name`; `name[k]` → `name_0..name_{k-1}`), in declared field order, so output
//! tables are byte-identical to the old code for the GE 2-photon (`events_2p`) and 3-photon
//! (`events_3p`) records. New record layouts (singles, coin_*p, future vendor compounds) read with
//! zero new code.
//!
//! The 3-photon record (verified via `h5ls -v` on real DUPLET data): `ms` u32 (timestamp), `id[3]`
//! u16 (crystal ids), `en[3]` f32 (energies), `vtx[3]` f32 (vertex), `lt` f32 (o-Ps lifetime) — 38
//! B/record. The 2-photon record (verified via `h5dump -H` on real DUPLET data — a *different*
//! member set): `ms` u32 · `en[2]` f32 · `ax[2]` u8 · `tx[2]` u16 · `vtx[3]` f32 — 30 B/record.
//! libhdf5 is found via pkg-config (see flake.nix; HDF5_DIR unset).
//!
//! **Memory ceiling:** [`read_compound`] reads the *whole* compound dataset into RAM, then encodes
//! one table block — fine for moderate acquisitions, but a multi-GB `events_2p` (~10⁸ rows) needs a
//! bounded-memory, row-group-streaming path: that's [`stream_compound`] (ADR-0026 §3).

use hdf5::types::{CompoundField, CompoundType, FloatSize, IntSize, TypeDescriptor};
use hdf5::Dataset;
use hdf5_metno as hdf5;
use hdf5_metno_sys::h5::hsize_t;
use hdf5_metno_sys::h5d::{H5Dget_type, H5Dread};
use hdf5_metno_sys::h5i::hid_t;
use hdf5_metno_sys::h5p::H5P_DEFAULT;
use hdf5_metno_sys::h5s::{
    H5Sclose, H5Screate_simple, H5Sselect_hyperslab, H5S_ALL, H5S_SELECT_SET,
};
use hdf5_metno_sys::h5t::H5Tclose;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::table::{self, ColumnData, TableData};
use tessera_io::BlockPayload;

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ge-hdf5: {e}"))
}

/// Get the [`CompoundType`] descriptor of a compound dataset (the per-field offset/type table the
/// generic reader slices columns through). Returns an error if the dataset isn't a compound.
fn compound_descriptor(ds: &Dataset) -> Result<CompoundType> {
    let dt = ds.dtype().map_err(he)?;
    match dt.to_descriptor().map_err(he)? {
        TypeDescriptor::Compound(c) => Ok(c),
        other => Err(he(format!(
            "dataset datatype is not a compound (got {other})"
        ))),
    }
}

/// The fd5 numpy-style dtype code for a *scalar* [`TypeDescriptor`] member — the integer/float widths
/// `ColumnData` represents natively. Other shapes (bool/enum/strings/refs/nested compounds) are
/// rejected with an actionable error rather than silently dropped.
fn scalar_numpy_code(ty: &TypeDescriptor) -> Result<&'static str> {
    match ty {
        TypeDescriptor::Integer(IntSize::U1) => Ok("i1"),
        TypeDescriptor::Integer(IntSize::U2) => Ok("i2"),
        TypeDescriptor::Integer(IntSize::U4) => Ok("i4"),
        TypeDescriptor::Integer(IntSize::U8) => Ok("i8"),
        TypeDescriptor::Unsigned(IntSize::U1) => Ok("u1"),
        TypeDescriptor::Unsigned(IntSize::U2) => Ok("u2"),
        TypeDescriptor::Unsigned(IntSize::U4) => Ok("u4"),
        TypeDescriptor::Unsigned(IntSize::U8) => Ok("u8"),
        TypeDescriptor::Float(FloatSize::U4) => Ok("f4"),
        TypeDescriptor::Float(FloatSize::U8) => Ok("f8"),
        other => Err(he(format!(
            "compound field has unsupported scalar type {other} \
             (only i1/i2/i4/i8 u1/u2/u4/u8 f4/f8 — and fixed arrays of those — are decoded)"
        ))),
    }
}

/// Build an empty [`ColumnData`] of the dtype named by a numpy code (the slab accumulator).
fn empty_for(code: &str) -> Result<ColumnData> {
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
        other => Err(he(format!("internal: unknown numpy code '{other}'")))?,
    })
}

/// The flat column schema (name + numpy code) implied by a compound descriptor's declared field
/// order — scalar `field` keeps its name; fixed-array `field[k]` explodes to `field_0..field_{k-1}`.
/// This is the SSoT for the columns the generic reader produces (the same as the existing per-record
/// transposes, byte-identical for the GE 2p/3p layouts).
fn compound_column_schema(c: &CompoundType) -> Result<Vec<(String, &'static str)>> {
    // Field declaration order = increasing `index`; HDF5 returns members in declared order, but be
    // defensive against shuffled inputs (the descriptor exposes both `index` and physical offset).
    let mut fields: Vec<&CompoundField> = c.fields.iter().collect();
    fields.sort_by_key(|f| f.index);
    let mut schema = Vec::with_capacity(fields.len());
    for f in fields {
        match &f.ty {
            TypeDescriptor::FixedArray(inner, n) => {
                let code = scalar_numpy_code(inner)?;
                for k in 0..*n {
                    schema.push((format!("{}_{k}", f.name), code));
                }
            }
            ty => schema.push((f.name.clone(), scalar_numpy_code(ty)?)),
        }
    }
    Ok(schema)
}

/// Decode `n_rows` of a row-major compound buffer (`bytes` == `n_rows * compound.size` bytes,
/// laid out per the on-disk datatype) into flat columns — scalar fields read with `from_le_bytes`,
/// fixed-array fields explode to `name_0..` columns. The byte layout is the file datatype's, so
/// `bytes` must come from an **identity-typed** H5Dread (file_type == memory_type), as both
/// [`read_compound`] and [`stream_compound`] arrange.
///
/// Endianness: GE listmode is little-endian (verified on the DUPLET corpus); the descriptor doesn't
/// expose per-field byte-order in the public API, so we parse LE. Native x86_64 / aarch64 are LE,
/// so this also matches what the old `read_raw::<RecXp>` path produced — guaranteed by the
/// byte-identical proof tests below.
fn slab_to_columns(bytes: &[u8], c: &CompoundType, n_rows: usize) -> Result<TableData> {
    let rec = c.size;
    if rec.checked_mul(n_rows) != Some(bytes.len()) {
        return Err(he(format!(
            "slab byte len {} != n_rows {n_rows} * record {rec}",
            bytes.len()
        )));
    }
    let mut fields: Vec<&CompoundField> = c.fields.iter().collect();
    fields.sort_by_key(|f| f.index);

    let schema = compound_column_schema(c)?;
    let mut cols: Vec<ColumnData> = schema
        .iter()
        .map(|(_, code)| empty_for(code))
        .collect::<Result<_>>()?;
    for col in &mut cols {
        match col {
            ColumnData::I8(v) => v.reserve_exact(n_rows),
            ColumnData::I16(v) => v.reserve_exact(n_rows),
            ColumnData::I32(v) => v.reserve_exact(n_rows),
            ColumnData::I64(v) => v.reserve_exact(n_rows),
            ColumnData::U8(v) => v.reserve_exact(n_rows),
            ColumnData::U16(v) => v.reserve_exact(n_rows),
            ColumnData::U32(v) => v.reserve_exact(n_rows),
            ColumnData::U64(v) => v.reserve_exact(n_rows),
            ColumnData::F32(v) => v.reserve_exact(n_rows),
            ColumnData::F64(v) => v.reserve_exact(n_rows),
        }
    }

    // For each record, walk the field plan and push each (sub-)scalar.
    for r in 0..n_rows {
        let base = r * rec;
        let mut out_idx = 0usize;
        for f in &fields {
            match &f.ty {
                TypeDescriptor::FixedArray(inner, n) => {
                    let elem = inner.size();
                    for k in 0..*n {
                        let off = base + f.offset + k * elem;
                        push_scalar(&mut cols[out_idx], inner, &bytes[off..off + elem])?;
                        out_idx += 1;
                    }
                }
                ty => {
                    let off = base + f.offset;
                    let sz = ty.size();
                    push_scalar(&mut cols[out_idx], ty, &bytes[off..off + sz])?;
                    out_idx += 1;
                }
            }
        }
    }

    Ok(schema.into_iter().map(|(n, _)| n).zip(cols).collect())
}

/// Decode one LE scalar from `b` (exactly `ty.size()` bytes) and push onto `col` — the inner loop
/// of [`slab_to_columns`]. Width mismatches between `col` and `ty` indicate an internal logic bug
/// (the column was allocated from the same descriptor), so they're an `Error::Invalid` rather than a
/// panic.
fn push_scalar(col: &mut ColumnData, ty: &TypeDescriptor, b: &[u8]) -> Result<()> {
    macro_rules! le {
        ($t:ty, $b:expr) => {
            <$t>::from_le_bytes(<[u8; std::mem::size_of::<$t>()]>::try_from($b).map_err(he)?)
        };
    }
    match (col, ty) {
        (ColumnData::I8(v), TypeDescriptor::Integer(IntSize::U1)) => v.push(b[0] as i8),
        (ColumnData::I16(v), TypeDescriptor::Integer(IntSize::U2)) => v.push(le!(i16, b)),
        (ColumnData::I32(v), TypeDescriptor::Integer(IntSize::U4)) => v.push(le!(i32, b)),
        (ColumnData::I64(v), TypeDescriptor::Integer(IntSize::U8)) => v.push(le!(i64, b)),
        (ColumnData::U8(v), TypeDescriptor::Unsigned(IntSize::U1)) => v.push(b[0]),
        (ColumnData::U16(v), TypeDescriptor::Unsigned(IntSize::U2)) => v.push(le!(u16, b)),
        (ColumnData::U32(v), TypeDescriptor::Unsigned(IntSize::U4)) => v.push(le!(u32, b)),
        (ColumnData::U64(v), TypeDescriptor::Unsigned(IntSize::U8)) => v.push(le!(u64, b)),
        (ColumnData::F32(v), TypeDescriptor::Float(FloatSize::U4)) => v.push(le!(f32, b)),
        (ColumnData::F64(v), TypeDescriptor::Float(FloatSize::U8)) => v.push(le!(f64, b)),
        (c, t) => {
            return Err(he(format!(
                "internal: column dtype {} doesn't match field type {t}",
                c.numpy_code()
            )))
        }
    }
    Ok(())
}

/// RAII guard that closes a libhdf5 hid_t via the given `close` function on drop — keeps the FFI
/// blocks below exception-safe even though no Rust panics cross the boundary (any error path simply
/// drops the guard and the resource is released). The close itself runs under hdf5-metno's global
/// reentrant lock (see [`Drop`]), so a guard may be dropped **outside** the `sync()` block that
/// created its id and the close is still serialized against concurrent libhdf5 calls.
struct H5Id {
    id: hid_t,
    close: unsafe extern "C" fn(hid_t) -> i32,
}
impl Drop for H5Id {
    fn drop(&mut self) {
        if self.id > 0 {
            // SAFETY: `id` is a libhdf5 identifier we own (from H5Dget_type / H5Dget_space /
            // H5Screate_simple); `close` matches its kind. The close runs inside `hdf5::sync::sync`
            // — hdf5-metno's process-global **reentrant** lock (`pub` but `#[doc(hidden)]`; the
            // intentional escape hatch its own `h5call!`/`h5lock!` macros use) — so it can't race a
            // concurrent libhdf5 call on another thread (libhdf5 is built non-thread-safe), and
            // reentrancy makes it safe even when this drops inside an enclosing `sync()` block.
            // Failures are ignored: Drop can't propagate, worst case a brief id leak on a bad file.
            hdf5::sync::sync(|| unsafe {
                let _ = (self.close)(self.id);
            });
        }
    }
}

/// Read the whole compound dataset (any record layout) and transpose to flat Tessera columns —
/// the generic-reader entry point. Fixed-array members fan out as `name_0..name_{N-1}`; scalar
/// members keep their name. Column ORDER == the compound's declared field order, so the output is
/// **byte-identical** to the old hardcoded GE 2p/3p paths (and to any future record type added).
pub fn read_compound(path: &std::path::Path, dataset: &str) -> Result<TableData> {
    let file = hdf5::File::open(path).map_err(he)?;
    let ds = file.dataset(dataset).map_err(he)?;
    let c = compound_descriptor(&ds)?;
    let n = ds.shape().first().copied().unwrap_or(0);
    let mut buf = vec![0u8; n.checked_mul(c.size).ok_or_else(|| he("read overflow"))?];

    // SAFETY: `ds.id()` is a live HDF5 dataset id (held alive by `ds`); H5Dget_type yields a fresh
    // type id we own (closed by the RAII guard); we pass that same type id for both memory and file
    // type so HDF5 does an identity-byte memcpy (no conversion). H5S_ALL for both spaces reads every
    // row. `buf.len()` == `n * compound.size`, so the destination is exactly the right size. The
    // whole FFI block runs under `hdf5::sync::sync()` — the same recursive mutex hdf5-metno wraps
    // its own h5call! / h5lock! macros with — so our raw calls can't race with concurrent
    // hdf5-metno operations on other threads (libhdf5 isn't thread-safe by default).
    hdf5::sync::sync(|| -> Result<()> {
        unsafe {
            let dt_id = H5Dget_type(ds.id());
            if dt_id < 0 {
                return Err(he("H5Dget_type failed"));
            }
            let _dt = H5Id {
                id: dt_id,
                close: H5Tclose,
            };
            let rc = H5Dread(
                ds.id(),
                dt_id,
                H5S_ALL,
                H5S_ALL,
                H5P_DEFAULT,
                buf.as_mut_ptr().cast(),
            );
            if rc < 0 {
                return Err(he("H5Dread failed"));
            }
        }
        Ok(())
    })?;

    slab_to_columns(&buf, &c, n)
}

/// Default row-slab for streaming reads — rows pulled per HDF5 hyperslab (the bounded-memory unit).
pub const STREAM_SLAB_ROWS: usize = 1 << 16;

/// Stream any compound dataset in bounded-memory **row-slabs** (ADR-0026 §3): read `slab_rows`
/// records at a time via an HDF5 hyperslab, decode each slab to columns with [`slab_to_columns`],
/// and feed `sink` — never holding more than one slab in RAM. `slab_rows` clamps to ≥ 1. Same
/// per-slab decoder as the whole-file [`read_compound`] → identical columns when row-aligned, so
/// streaming-then-concatenate equals the whole-file table (proved by the byte-identical tests).
pub fn stream_compound(
    path: &std::path::Path,
    dataset: &str,
    slab_rows: usize,
    mut sink: impl FnMut(TableData) -> Result<()>,
) -> Result<()> {
    let file = hdf5::File::open(path).map_err(he)?;
    let ds = file.dataset(dataset).map_err(he)?;
    let c = compound_descriptor(&ds)?;
    let n = ds.shape().first().copied().unwrap_or(0);
    let slab = slab_rows.max(1);

    // Acquire libhdf5 type + file-space ids once (under `sync()`); they're reused for every slab.
    let (dt_id, file_space_id) = hdf5::sync::sync(|| -> Result<(hid_t, hid_t)> {
        // SAFETY: the dataset id is live (held by `ds`). H5Dget_type returns a fresh file-type id we
        // own, and H5Dget_space a fresh dataspace id we own — both released by the H5Id RAII guards
        // below. On the get-space error path we close the already-acquired type id before returning.
        unsafe {
            let dt_id = H5Dget_type(ds.id());
            if dt_id < 0 {
                return Err(he("H5Dget_type failed"));
            }
            let file_space_id = hdf5_metno_sys::h5d::H5Dget_space(ds.id());
            if file_space_id < 0 {
                hdf5_metno_sys::h5t::H5Tclose(dt_id);
                return Err(he("H5Dget_space failed"));
            }
            Ok((dt_id, file_space_id))
        }
    })?;
    // These two guards outlive the acquisition `sync()` block and drop at function return (after the
    // slab loop) — that's sound because `H5Id::drop` re-takes the reentrant lock itself, so the
    // H5Tclose/H5Sclose are serialized against concurrent libhdf5 calls regardless of drop site.
    let _dt = H5Id {
        id: dt_id,
        close: H5Tclose,
    };
    let _fs = H5Id {
        id: file_space_id,
        close: H5Sclose,
    };

    let mut start = 0usize;
    while start < n {
        let end = (start + slab).min(n);
        let count = end - start;
        let mut buf = vec![
            0u8;
            count
                .checked_mul(c.size)
                .ok_or_else(|| he("read overflow"))?
        ];

        // SAFETY: `dt_id` and `file_space_id` are alive (RAII guards); each iteration creates a
        // fresh memory dataspace matching `count` rows (rank 1) and closes it before exit; the
        // identity-typed H5Dread copies exactly `count * compound.size` bytes verbatim into `buf`
        // (whose len matches). The whole FFI block runs under hdf5-metno's recursive mutex.
        hdf5::sync::sync(|| -> Result<()> {
            let start_h = [u64::try_from(start).map_err(he)? as hsize_t];
            let count_h = [u64::try_from(count).map_err(he)? as hsize_t];
            unsafe {
                let rc = H5Sselect_hyperslab(
                    file_space_id,
                    H5S_SELECT_SET,
                    start_h.as_ptr(),
                    std::ptr::null(),
                    count_h.as_ptr(),
                    std::ptr::null(),
                );
                if rc < 0 {
                    return Err(he("H5Sselect_hyperslab failed"));
                }
                let mem_space_id = H5Screate_simple(1, count_h.as_ptr(), std::ptr::null());
                if mem_space_id < 0 {
                    return Err(he("H5Screate_simple failed"));
                }
                let _ms = H5Id {
                    id: mem_space_id,
                    close: H5Sclose,
                };
                let rc = H5Dread(
                    ds.id(),
                    dt_id,
                    mem_space_id,
                    file_space_id,
                    H5P_DEFAULT,
                    buf.as_mut_ptr().cast(),
                );
                if rc < 0 {
                    return Err(he("H5Dread failed"));
                }
            }
            Ok(())
        })?;

        let slab_cols = slab_to_columns(&buf, &c, count)?;
        sink(slab_cols)?;
        start = end;
    }

    Ok(())
}

/// Build a `Vec<Column>` (name + numpy code) for the compound dataset — the schema half of the
/// generic reader, used to construct a [`TableSpec`] without materialising the table. Cheap: opens
/// the dataset, reads its datatype descriptor, closes.
pub fn compound_columns(path: &std::path::Path, dataset: &str) -> Result<Vec<Column>> {
    let file = hdf5::File::open(path).map_err(he)?;
    let ds = file.dataset(dataset).map_err(he)?;
    let c = compound_descriptor(&ds)?;
    Ok(compound_column_schema(&c)?
        .into_iter()
        .map(|(name, code)| Column {
            name,
            dtype: code.into(),
            codec: None,
        })
        .collect())
}

/// Read a GE listmode 3-photon dataset (default name `events_3p`) and transpose to flat Tessera
/// columns: `ms` (u4) · `id_0..2` (u2) · `en_0..2` (f4) · `vtx_0..2` (f4) · `lt` (f4). Thin GE-named
/// alias for [`read_compound`] — the actual decode is generic.
pub fn read_events_3p(path: &std::path::Path, dataset: &str) -> Result<TableData> {
    read_compound(path, dataset)
}

/// Read a GE listmode 2-photon dataset (default name `events_2p`) and transpose to flat Tessera
/// columns: `ms` (u4) · `en_0..1` (f4) · `ax_0..1` (u1) · `tx_0..1` (u2) · `vtx_0..2` (f4). Thin
/// GE-named alias for [`read_compound`] — the actual decode is generic.
pub fn read_events_2p(path: &std::path::Path, dataset: &str) -> Result<TableData> {
    read_compound(path, dataset)
}

/// Stream a GE 2-photon dataset in bounded-memory row-slabs (the >RAM path). Thin alias for the
/// generic [`stream_compound`].
pub fn stream_events_2p(
    path: &std::path::Path,
    dataset: &str,
    slab_rows: usize,
    sink: impl FnMut(TableData) -> Result<()>,
) -> Result<()> {
    stream_compound(path, dataset, slab_rows, sink)
}

/// Bounded-memory ingest of a GE 2-photon dataset → sealed `listmode` product (ADR-0026 §3): stream
/// the HDF5 in row-slabs through the [`tessera_io::TableStreamWriter`] (≈ one slab on read + ≈ one
/// row-group on encode), staging fragments under `stage`, then seal. **Byte-identical** to the
/// whole-file [`read_events_2p`] → [`to_listmode_product`] path (the canonical row-group grid is
/// independent of the slab size), but never materialises the whole acquisition.
pub fn stream_to_listmode_product_2p(
    path: &std::path::Path,
    dataset: &str,
    name: &str,
    timestamp: &str,
    slab_rows: usize,
    stage: &std::path::Path,
) -> Result<(Manifest, Vec<BlockPayload>)> {
    // Schema is one cheap HDF5 open; the streaming pass below opens the file again to read the data.
    let columns = compound_columns(path, dataset)?;
    let n = {
        let file = hdf5::File::open(path).map_err(he)?;
        u64::try_from(
            file.dataset(dataset)
                .map_err(he)?
                .shape()
                .first()
                .copied()
                .unwrap_or(0),
        )
        .map_err(he)?
    };
    let spec = TableSpec {
        columns,
        rows: n,
        row_index: Some("ms".into()),
    };
    let mut w = tessera_io::TableStreamWriter::new(spec.clone(), stage)?;
    stream_compound(path, dataset, slab_rows, |slab| w.push(slab))?;
    let bytes = w.finish()?;

    let digest = tessera_core::hash::digest(&bytes);
    let block_ref = tessera_core::block::BlockRef {
        name: "events".into(),
        kind: tessera_core::block::BlockKind::Table,
        digest: Some(digest),
        spec: serde_json::to_value(&spec).map_err(he)?,
    };
    let mut b = ProductBuilder::new(
        "listmode",
        name,
        "GE listmode coincidence events (streamed)",
        timestamp,
    );
    b.add_block_ref(block_ref);
    b.add_source(tessera_core::provenance::Source::new(
        "ingested_from",
        path.display().to_string(),
    ));
    let sealed = b.seal()?;
    Ok((sealed, vec![BlockPayload::new("events", bytes)]))
}

/// Build a sealed Tessera `listmode` product (Vortex table) from flattened GE columns, with an
/// `ingested_from` provenance edge to the source `.h5`. `row_index` is `ms`.
pub fn to_listmode_product(
    cols: &TableData,
    name: &str,
    timestamp: &str,
    source: &str,
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let rows = u64::try_from(cols.first().map(|(_, c)| c.len()).unwrap_or(0)).map_err(he)?;
    let columns: Vec<Column> = cols
        .iter()
        .map(|(n, c)| Column {
            name: n.clone(),
            dtype: c.numpy_code().into(),
            codec: None,
        })
        .collect();
    let spec = TableSpec {
        columns,
        rows,
        row_index: Some("ms".into()),
    };
    let (block_ref, payload) = table::table_block("events", &spec, cols)?;
    let mut b = ProductBuilder::new(
        "listmode",
        name,
        "GE listmode coincidence events",
        timestamp,
    );
    b.add_block_ref(block_ref);
    b.add_source(tessera_core::provenance::Source::new(
        "ingested_from",
        source,
    ));
    let sealed = b.seal()?;
    Ok((sealed, vec![payload]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hdf5::H5Type;

    // Fixture-write only: writing a synthetic .h5 with a compound dataset legitimately needs a
    // compile-time `H5Type` (the `new_dataset::<T>()` builder is generic). The READER under test
    // (`read_compound` / `stream_compound`) never sees these structs.

    /// GE listmode 3-photon record (`Rec3p`) — fixture-write only, mirrors the on-disk member set.
    #[repr(C)]
    #[derive(H5Type, Clone, Copy)]
    struct Rec3p {
        ms: u32,
        id: [u16; 3],
        en: [f32; 3],
        vtx: [f32; 3],
        lt: f32,
    }

    /// GE listmode 2-photon record (`Rec2p`) — fixture-write only, mirrors the on-disk member set.
    #[repr(C)]
    #[derive(H5Type, Clone, Copy)]
    struct Rec2p {
        ms: u32,
        en: [f32; 2],
        ax: [u8; 2],
        tx: [u16; 2],
        vtx: [f32; 3],
    }

    fn write_synth_3p(path: &std::path::Path, n: usize) {
        let recs: Vec<Rec3p> = (0..n)
            .map(|k| Rec3p {
                ms: k as u32,
                id: [k as u16, k as u16 + 1, k as u16 + 2],
                en: [511.0, 510.0 + k as f32, 512.0],
                vtx: [0.1 * k as f32, 0.2, 0.3],
                lt: 1.5 + k as f32 * 0.01,
            })
            .collect();
        let f = hdf5::File::create(path).unwrap();
        f.new_dataset::<Rec3p>()
            .shape(n)
            .create("events_3p")
            .unwrap()
            .write(&recs)
            .unwrap();
    }

    fn write_synth_2p(path: &std::path::Path, n: usize) {
        let recs: Vec<Rec2p> = (0..n)
            .map(|k| Rec2p {
                ms: k as u32,
                en: [511.0, 510.0 + k as f32],
                ax: [(k % 64) as u8, (k % 32) as u8],
                tx: [k as u16, k as u16 + 7],
                vtx: [0.1 * k as f32, 0.2, 0.3],
            })
            .collect();
        let f = hdf5::File::create(path).unwrap();
        f.new_dataset::<Rec2p>()
            .shape(n)
            .create("events_2p")
            .unwrap()
            .write(&recs)
            .unwrap();
    }

    // The hardcoded transposes from the previous implementation, kept ONLY for the byte-identical
    // ground-truth proof (the new generic reader must match these column-for-column on the GE
    // record layouts the spike fixed in code). Production code no longer carries these — the
    // generic `read_compound` replaces both.

    fn ground_truth_3p(recs: &[Rec3p]) -> TableData {
        let n = recs.len();
        let mut ms = Vec::with_capacity(n);
        let mut id: [Vec<u16>; 3] = Default::default();
        let mut en: [Vec<f32>; 3] = Default::default();
        let mut vtx: [Vec<f32>; 3] = Default::default();
        let mut lt = Vec::with_capacity(n);
        for r in recs {
            ms.push(r.ms);
            lt.push(r.lt);
            for k in 0..3 {
                id[k].push(r.id[k]);
                en[k].push(r.en[k]);
                vtx[k].push(r.vtx[k]);
            }
        }
        let mut cols: TableData = vec![("ms".into(), ColumnData::U32(ms))];
        for (k, v) in id.into_iter().enumerate() {
            cols.push((format!("id_{k}"), ColumnData::U16(v)));
        }
        for (k, v) in en.into_iter().enumerate() {
            cols.push((format!("en_{k}"), ColumnData::F32(v)));
        }
        for (k, v) in vtx.into_iter().enumerate() {
            cols.push((format!("vtx_{k}"), ColumnData::F32(v)));
        }
        cols.push(("lt".into(), ColumnData::F32(lt)));
        cols
    }

    fn ground_truth_2p(recs: &[Rec2p]) -> TableData {
        let n = recs.len();
        let mut ms = Vec::with_capacity(n);
        let mut en: [Vec<f32>; 2] = Default::default();
        let mut ax: [Vec<u8>; 2] = Default::default();
        let mut tx: [Vec<u16>; 2] = Default::default();
        let mut vtx: [Vec<f32>; 3] = Default::default();
        for r in recs {
            ms.push(r.ms);
            for k in 0..2 {
                en[k].push(r.en[k]);
                ax[k].push(r.ax[k]);
                tx[k].push(r.tx[k]);
            }
            for (k, &v) in r.vtx.iter().enumerate() {
                vtx[k].push(v);
            }
        }
        let mut cols: TableData = vec![("ms".into(), ColumnData::U32(ms))];
        for (k, v) in en.into_iter().enumerate() {
            cols.push((format!("en_{k}"), ColumnData::F32(v)));
        }
        for (k, v) in ax.into_iter().enumerate() {
            cols.push((format!("ax_{k}"), ColumnData::U8(v)));
        }
        for (k, v) in tx.into_iter().enumerate() {
            cols.push((format!("tx_{k}"), ColumnData::U16(v)));
        }
        for (k, v) in vtx.into_iter().enumerate() {
            cols.push((format!("vtx_{k}"), ColumnData::F32(v)));
        }
        cols
    }

    #[test]
    fn generic_read_compound_byte_identical_to_hardcoded_2p_and_3p() {
        // #222 column-level byte-identical proof: the generic descriptor-driven reader produces
        // EXACTLY the same TableData (names, dtypes, values, order) as the old hardcoded per-record
        // transposes on the same .h5 fixture — for both the 2p AND 3p compound layouts.
        let dir = tempfile::tempdir().unwrap();

        let h5_3p = dir.path().join("LIST_3p.h5");
        write_synth_3p(&h5_3p, 257); // odd, non-multiple
        let recs_3p: Vec<Rec3p> = (0..257usize)
            .map(|k| Rec3p {
                ms: k as u32,
                id: [k as u16, k as u16 + 1, k as u16 + 2],
                en: [511.0, 510.0 + k as f32, 512.0],
                vtx: [0.1 * k as f32, 0.2, 0.3],
                lt: 1.5 + k as f32 * 0.01,
            })
            .collect();
        let expect_3p = ground_truth_3p(&recs_3p);
        let got_3p = read_compound(&h5_3p, "events_3p").unwrap();
        assert_eq!(got_3p, expect_3p, "generic reader diverged on 3p compound");

        let h5_2p = dir.path().join("LIST_2p.h5");
        write_synth_2p(&h5_2p, 1001);
        let recs_2p: Vec<Rec2p> = (0..1001usize)
            .map(|k| Rec2p {
                ms: k as u32,
                en: [511.0, 510.0 + k as f32],
                ax: [(k % 64) as u8, (k % 32) as u8],
                tx: [k as u16, k as u16 + 7],
                vtx: [0.1 * k as f32, 0.2, 0.3],
            })
            .collect();
        let expect_2p = ground_truth_2p(&recs_2p);
        let got_2p = read_compound(&h5_2p, "events_2p").unwrap();
        assert_eq!(got_2p, expect_2p, "generic reader diverged on 2p compound");
    }

    #[test]
    fn stream_compound_concatenated_matches_whole_file() {
        // The generic streaming path's slab decode == the whole-file decode when row-aligned.
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST_2p.h5");
        write_synth_2p(&h5, 5000);

        let whole = read_compound(&h5, "events_2p").unwrap();
        let mut acc: Option<TableData> = None;
        stream_compound(&h5, "events_2p", 999, |slab| {
            match acc.as_mut() {
                None => acc = Some(slab),
                Some(a) => {
                    for (i, (_, c)) in slab.into_iter().enumerate() {
                        a[i].1.extend(&c)?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(acc.unwrap(), whole, "streamed slabs != whole-file");
    }

    #[test]
    fn ingest_3p_transposes_to_columns_and_builds_listmode() {
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST_events.h5");
        write_synth_3p(&h5, 100);

        let cols = read_events_3p(&h5, "events_3p").unwrap();
        // 1 ms + 3 id + 3 en + 3 vtx + 1 lt = 11 columns, each 100 rows
        assert_eq!(cols.len(), 11);
        assert!(cols.iter().all(|(_, c)| c.len() == 100));
        let names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names[0], "ms");
        assert!(
            names.contains(&"id_2")
                && names.contains(&"en_0")
                && names.contains(&"vtx_1")
                && names.contains(&"lt")
        );
        // ms is the monotonic event index
        let ColumnData::U32(ms) = &cols[0].1 else {
            panic!()
        };
        assert_eq!(ms[0], 0);
        assert_eq!(ms[99], 99);

        let (sealed, payloads) = to_listmode_product(
            &cols,
            "DP06-lm",
            "2024-01-01T00:00:00Z",
            "LIST0000_events.h5",
        )
        .unwrap();
        assert_eq!(sealed.product, "listmode");
        assert_eq!(sealed.metadata.get("modality"), None);
        assert_eq!(sealed.sources[0].reference, "LIST0000_events.h5");

        let out = dir.path().join("lm.tsra");
        tessera_io::pack(&sealed, &payloads, &out).unwrap();
        let mut r = tessera_io::Reader::open(&out).unwrap();
        // round-trips through Vortex: decode the events table back
        let bytes = r.read_block("events").unwrap();
        let back = table::decode(
            &tessera_core::block::table::TableSpec {
                columns: cols
                    .iter()
                    .map(|(n, c)| Column {
                        name: n.clone(),
                        dtype: c.numpy_code().into(),
                        codec: None,
                    })
                    .collect(),
                rows: 100,
                row_index: Some("ms".into()),
            },
            &bytes,
        )
        .unwrap();
        assert_eq!(back, cols);
    }

    #[test]
    fn ingest_2p_transposes_distinct_member_set_to_columns() {
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST_2p.h5");
        write_synth_2p(&h5, 64);

        let cols = read_events_2p(&h5, "events_2p").unwrap();
        // 1 ms + 2 en + 2 ax + 2 tx + 3 vtx = 10 columns, each 64 rows
        assert_eq!(cols.len(), 10);
        assert!(cols.iter().all(|(_, c)| c.len() == 64));
        let by: std::collections::HashMap<&str, &ColumnData> =
            cols.iter().map(|(n, c)| (n.as_str(), c)).collect();
        // the 2p member set differs from 3p: ax (u1) + tx (u2), no id/lt
        assert_eq!(by["ax_0"].numpy_code(), "u1");
        assert_eq!(by["tx_1"].numpy_code(), "u2");
        assert_eq!(by["vtx_2"].numpy_code(), "f4");
        assert!(!by.contains_key("lt") && !by.contains_key("id_0"));
        let ColumnData::U8(ax0) = by["ax_0"] else {
            panic!()
        };
        assert_eq!(ax0[63], 63);

        // round-trips through the Vortex table backend (u8 column included)
        let (sealed, payloads) = to_listmode_product(
            &cols,
            "DP06-2p",
            "2024-01-01T00:00:00Z",
            "LIST0000_events.h5",
        )
        .unwrap();
        let out = dir.path().join("lm2p.tsra");
        tessera_io::pack(&sealed, &payloads, &out).unwrap();
        let mut r = tessera_io::Reader::open(&out).unwrap();
        let bytes = r.read_block("events").unwrap();
        let spec = TableSpec {
            columns: cols
                .iter()
                .map(|(n, c)| Column {
                    name: n.clone(),
                    dtype: c.numpy_code().into(),
                    codec: None,
                })
                .collect(),
            rows: 64,
            row_index: Some("ms".into()),
        };
        assert_eq!(table::decode(&spec, &bytes).unwrap(), cols);
    }

    #[test]
    fn streamed_2p_ingest_is_byte_identical_to_whole_file_read() {
        // ADR-0026 §3: bounded-memory row-slab streaming → the SAME sealed product as the whole-file read.
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST_2p.h5");
        write_synth_2p(&h5, 5000); // > one slab below

        // streamed: a deliberately small 999-row slab (NOT aligned to the 65536 row-group grid) so
        // multiple hyperslab reads + spill + compaction all run — the bounded-memory path.
        let (streamed, _p) = stream_to_listmode_product_2p(
            &h5,
            "events_2p",
            "DP06-2p",
            "2024-01-01T00:00:00Z",
            999,
            &dir.path().join("stage"),
        )
        .unwrap();
        streamed.verify().unwrap();

        // whole-file batch path over the same data.
        let cols = read_events_2p(&h5, "events_2p").unwrap();
        let (batch, _p) = to_listmode_product(
            &cols,
            "DP06-2p",
            "2024-01-01T00:00:00Z",
            &h5.display().to_string(),
        )
        .unwrap();

        // byte-identical content: the slab size never changes the canonical row-group bytes.
        assert_eq!(streamed.content_hash, batch.content_hash);
    }
}
