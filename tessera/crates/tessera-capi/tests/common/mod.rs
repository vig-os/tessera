//! Shared by the tessera-capi tests: a listmode-like batch, its C Data Interface export, and thin
//! wrappers that call the `extern "C"` surface exactly as a C caller does.
#![allow(dead_code)] // each test binary uses its own subset

use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::Arc;

use arrow_array::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use arrow_array::{Array, ArrayRef, Float32Array, StructArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Fields};
use tessera_capi::*;
use tessera_core::block::table::{Column, TableSpec};
use tessera_io::table::{ColumnData, TableData};

/// The listmode-like columns, the same functions of the global row index as `cpp/listmode.hpp`.
pub fn t_of(i: u64) -> u64 {
    i * 7919 + 3
}
pub fn crystal_a_of(i: u64) -> u32 {
    ((i.wrapping_mul(2654435761)) % 20160) as u32
}
pub fn crystal_b_of(i: u64) -> u32 {
    ((i * 40503 + 17) % 20160) as u32
}
pub fn energy_of(i: u64) -> f32 {
    ((i * 7) % 6500) as f32 * 0.1 + 0.05
}
pub fn tof_valid(i: u64) -> bool {
    !i.is_multiple_of(13)
}
pub fn tof_of(i: u64) -> f32 {
    ((i % 801) as i64 - 400) as f32 * 1.5
}

pub fn fields() -> Fields {
    Fields::from(vec![
        Field::new("t", DataType::UInt64, false),
        Field::new("crystal_a", DataType::UInt32, false),
        Field::new("crystal_b", DataType::UInt32, false),
        Field::new("energy", DataType::Float32, false),
        Field::new("tof", DataType::Float32, true),
    ])
}

/// Rows `[first, first + n)` as a struct array.
pub fn batch(first: u64, n: u64) -> StructArray {
    let r = first..first + n;
    let tof: Float32Array = r.clone().map(|i| tof_valid(i).then(|| tof_of(i))).collect();
    let cols: Vec<ArrayRef> = vec![
        Arc::new(UInt64Array::from_iter_values(r.clone().map(t_of))),
        Arc::new(UInt32Array::from_iter_values(r.clone().map(crystal_a_of))),
        Arc::new(UInt32Array::from_iter_values(r.clone().map(crystal_b_of))),
        Arc::new(Float32Array::from_iter_values(r.map(energy_of))),
        Arc::new(tof),
    ];
    StructArray::new(fields(), cols, None)
}

/// The same rows as the writer's native input, for the native-`push_table` comparison.
pub fn native_groups(rows: u64, group: u64) -> Vec<TableData> {
    (0..rows)
        .step_by(group as usize)
        .map(|first| {
            let r = first..(first + group).min(rows);
            vec![
                ("t".into(), ColumnData::U64(r.clone().map(t_of).collect())),
                (
                    "crystal_a".into(),
                    ColumnData::U32(r.clone().map(crystal_a_of).collect()),
                ),
                (
                    "crystal_b".into(),
                    ColumnData::U32(r.clone().map(crystal_b_of).collect()),
                ),
                (
                    "energy".into(),
                    ColumnData::F32(r.clone().map(energy_of).collect()),
                ),
                (
                    "tof".into(),
                    ColumnData::Nullable {
                        values: Box::new(ColumnData::F32(
                            r.clone()
                                .map(|i| if tof_valid(i) { tof_of(i) } else { 0.0 })
                                .collect(),
                        )),
                        validity: r.map(tof_valid).collect(),
                    },
                ),
            ]
        })
        .collect()
}

pub fn native_spec(rows: u64) -> TableSpec {
    TableSpec {
        columns: vec![
            Column::new("t", "u8"),
            Column::new("crystal_a", "u4"),
            Column::new("crystal_b", "u4"),
            Column::new("energy", "f4"),
            Column::new("tof", "f4").nullable(),
        ],
        rows,
        row_index: None,
        partition: None,
    }
}

/// The C Data Interface export of an array (what a C producer hands over).
pub fn export(a: &dyn Array) -> (FFI_ArrowArray, FFI_ArrowSchema) {
    arrow_array::ffi::to_ffi(&a.to_data()).expect("export")
}

pub fn schema_of(a: &dyn Array) -> FFI_ArrowSchema {
    FFI_ArrowSchema::try_from(a.data_type()).expect("schema")
}

pub fn last_error() -> Option<String> {
    let p = tsra_last_error();
    // SAFETY: the library returns NULL or a NUL-terminated string valid until the next call.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Assert a call's status, showing the library's message on a mismatch.
#[track_caller]
pub fn expect(status: TsraStatus, want: TsraStatus) {
    assert_eq!(status, want, "last error: {:?}", last_error());
}

pub const TS: &str = "2026-11-11T00:00:00Z";

pub struct Strings {
    product: CString,
    name: CString,
    description: CString,
    timestamp: CString,
}

impl Strings {
    pub fn new() -> Self {
        Strings {
            product: CString::new("listmode").unwrap(),
            name: CString::new("capi-test").unwrap(),
            description: CString::new("written through the C ABI").unwrap(),
            timestamp: CString::new(TS).unwrap(),
        }
    }
    pub fn opts(&self, workers: u32) -> TsraSessionOptions {
        TsraSessionOptions {
            struct_size: std::mem::size_of::<TsraSessionOptions>(),
            product: self.product.as_ptr(),
            name: self.name.as_ptr(),
            description: self.description.as_ptr(),
            timestamp: self.timestamp.as_ptr(),
            workers,
        }
    }
}

pub fn open(stage: &Path, workers: u32) -> *mut TsraSession {
    let s = Strings::new();
    let dir = CString::new(stage.to_str().unwrap()).unwrap();
    let mut session = std::ptr::null_mut();
    // SAFETY: valid strings and pointers for the call.
    expect(
        unsafe { tsra_session_open(dir.as_ptr(), &s.opts(workers), &mut session) },
        TsraStatus::Ok,
    );
    session
}

pub fn table_opts(rows: u64) -> TsraTableOptions {
    table_opts_window(rows, 0)
}

pub fn table_opts_window(rows: u64, reorder_window: u32) -> TsraTableOptions {
    table_opts_producers(rows, reorder_window, 0)
}

pub fn table_opts_producers(rows: u64, reorder_window: u32, producers: u32) -> TsraTableOptions {
    TsraTableOptions {
        struct_size: std::mem::size_of::<TsraTableOptions>(),
        rows,
        reorder_window,
        producers,
    }
}

pub fn begin(
    session: *mut TsraSession,
    schema: &FFI_ArrowSchema,
    rows: u64,
) -> (TsraStatus, *mut TsraTable) {
    begin_with(session, schema, &table_opts(rows))
}

pub fn begin_with(
    session: *mut TsraSession,
    schema: &FFI_ArrowSchema,
    opts: &TsraTableOptions,
) -> (TsraStatus, *mut TsraTable) {
    let name = CString::new("events").unwrap();
    let mut table = std::ptr::null_mut();
    // SAFETY: a live session, a valid schema and options, and an out pointer.
    let st = unsafe { tsra_table_begin(session, name.as_ptr(), schema, opts, &mut table) };
    (st, table)
}

/// Push `a` as batch `seq`; returns the status. The exported array is moved into the library.
pub fn push(table: *mut TsraTable, seq: u64, a: &dyn Array) -> TsraStatus {
    let (mut array, schema) = export(a);
    // SAFETY: a live table, an owned exported array, and its schema.
    let st = unsafe { tsra_table_push(table, seq, &mut array, &schema) };
    assert!(
        array.is_released(),
        "the library must take ownership of the array on every path"
    );
    st
}

pub fn end(table: *mut TsraTable) -> TsraStatus {
    // SAFETY: a live table with no push in flight.
    unsafe { tsra_table_end(table) }
}

/// Seal; returns the status and the content hash.
pub fn seal(session: *mut TsraSession, out: &Path) -> (TsraStatus, String) {
    let path = CString::new(out.to_str().unwrap()).unwrap();
    let mut hash = [0 as std::ffi::c_char; TSRA_HASH_CAPACITY];
    // SAFETY: a live session, a NUL-terminated path and a buffer of `hash.len()` bytes.
    let st = unsafe { tsra_session_seal(session, path.as_ptr(), hash.as_mut_ptr(), hash.len()) };
    // SAFETY: the library NUL-terminates what it writes; an untouched buffer is all NULs.
    let h = unsafe { CStr::from_ptr(hash.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (st, h)
}

/// Write `rows` rows through the C ABI in batches of `batch_rows`, in sequence order, declaring
/// `declared_rows` (or `TSRA_ROWS_UNKNOWN`); returns the content hash.
pub fn write_through_capi(dir: &Path, rows: u64, batch_rows: u64, declared_rows: u64) -> String {
    let session = open(&dir.join("stage"), 2);
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let (st, table) = begin(session, &schema, declared_rows);
    expect(st, TsraStatus::Ok);
    for (seq, first) in (0..rows).step_by(batch_rows as usize).enumerate() {
        let n = batch_rows.min(rows - first);
        expect(push(table, seq as u64, &batch(first, n)), TsraStatus::Ok);
    }
    expect(end(table), TsraStatus::Ok);
    let (st, hash) = seal(session, &dir.join("capi.tsra"));
    expect(st, TsraStatus::Ok);
    hash
}
