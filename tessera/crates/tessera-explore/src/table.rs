//! Derived views over a **table** block — the projected, windowed row page the CLI `read` (and later
//! the TUI / serve) render, over the cross-block logical view. Column decode stays in [`tessera_io`];
//! this module projects + windows the logical columns into an Arrow [`RecordBatch`] — the view-model's
//! one tabular result contract (shared with the aggregation primitives and a future `tsra serve`).
//! Row-selection *semantics* (negative/open bounds) stay with the caller via the `window` closure.

use std::io::{Read, Seek};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, Int8Array,
    UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use serde_json::Value;
use tessera_core::{Error, Result};
use tessera_io::{ColumnData, Reader};

/// A projected, windowed page of a logical table as an Arrow [`RecordBatch`], plus the full
/// (pre-window) row count for truncation reporting. `batch.num_rows()` is the page length;
/// `batch.schema()` the projected columns in output order.
pub struct TablePage {
    /// The projected, windowed columns as one Arrow record batch — the result contract.
    pub batch: RecordBatch,
    /// Total rows in the (logical) table, before windowing.
    pub total: u64,
}

/// Build an Arrow array from a decoded column. Dense — non-finite floats stay as float values (NaN /
/// ±inf), not nulls; the renderer decides how to display them.
pub fn column_to_arrow(col: &ColumnData) -> ArrayRef {
    match col {
        ColumnData::I8(v) => Arc::new(Int8Array::from(v.clone())),
        ColumnData::I16(v) => Arc::new(Int16Array::from(v.clone())),
        ColumnData::I32(v) => Arc::new(Int32Array::from(v.clone())),
        ColumnData::I64(v) => Arc::new(Int64Array::from(v.clone())),
        ColumnData::U8(v) => Arc::new(UInt8Array::from(v.clone())),
        ColumnData::U16(v) => Arc::new(UInt16Array::from(v.clone())),
        ColumnData::U32(v) => Arc::new(UInt32Array::from(v.clone())),
        ColumnData::U64(v) => Arc::new(UInt64Array::from(v.clone())),
        ColumnData::F32(v) => Arc::new(Float32Array::from(v.clone())),
        ColumnData::F64(v) => Arc::new(Float64Array::from(v.clone())),
    }
}

/// Read a projected, windowed page of a logical table (cross-block: a read of `events` spans every
/// `events_NNNN`) as an Arrow [`RecordBatch`]. `columns` empty = all columns in schema order; `window`
/// receives the total row count and returns the half-open `[lo, hi)` to materialise (the caller owns
/// row-selection semantics). Only the requested columns are decoded — but note a full logical column
/// is materialised per request *before* windowing, so a row window is truncation, not a streaming read
/// (chunk-pruned streaming paging is a later refinement).
pub fn read_page<R: Read + Seek>(
    reader: &mut Reader<R>,
    prefix: &str,
    columns: &[String],
    window: impl FnOnce(u64) -> (u64, u64),
) -> Result<TablePage> {
    let view = reader.logical_table(prefix)?;
    let total = view.row_count();

    // Resolve the column projection against the table schema (clear error on a typo).
    let all_names: Vec<String> = view.columns().iter().map(|c| c.name.clone()).collect();
    let selected: Vec<String> = if columns.is_empty() {
        all_names.clone()
    } else {
        for c in columns {
            if !all_names.iter().any(|n| n == c) {
                return Err(Error::Invalid(format!(
                    "no column '{c}' in '{prefix}' (columns: {})",
                    all_names.join(", ")
                )));
            }
        }
        columns.to_vec()
    };

    let (lo, hi) = window(total);
    let lo_us = usize::try_from(lo).map_err(|e| Error::Invalid(e.to_string()))?;
    let hi_us = usize::try_from(hi).map_err(|e| Error::Invalid(e.to_string()))?;

    let mut fields: Vec<Field> = Vec::with_capacity(selected.len());
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(selected.len());
    for name in &selected {
        let arr = column_to_arrow(&view.column(reader, name)?.slice(lo_us, hi_us));
        fields.push(Field::new(name.clone(), arr.data_type().clone(), false));
        arrays.push(arr);
    }
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(TablePage { batch, total })
}

/// Project a record batch to **column-major JSON cells** (`cells[col][row]`): ints → `Number`, floats
/// → their native shortest round-trip `Display` (an `f32` shows `0.01`, not its widened-`f64`
/// expansion) or `Value::Null` for NaN / ±inf. A convenience for text / JSON renderers (the CLI's
/// CSV / TSV / NDJSON); the [`RecordBatch`] itself is the primary contract.
pub fn page_cells(batch: &RecordBatch) -> Vec<Vec<Value>> {
    macro_rules! ints {
        ($arr:expr, $ty:ty) => {{
            let a = $arr.as_any().downcast_ref::<$ty>().unwrap();
            a.iter()
                .map(|x| x.map_or(Value::Null, Value::from))
                .collect()
        }};
    }
    macro_rules! flts {
        ($arr:expr, $ty:ty) => {{
            let a = $arr.as_any().downcast_ref::<$ty>().unwrap();
            a.iter()
                .map(|x| {
                    x.map_or(Value::Null, |v| {
                        v.to_string()
                            .parse::<serde_json::Number>()
                            .map_or(Value::Null, Value::Number)
                    })
                })
                .collect()
        }};
    }
    batch
        .columns()
        .iter()
        .map(|arr| match arr.data_type() {
            DataType::Int8 => ints!(arr, Int8Array),
            DataType::Int16 => ints!(arr, Int16Array),
            DataType::Int32 => ints!(arr, Int32Array),
            DataType::Int64 => ints!(arr, Int64Array),
            DataType::UInt8 => ints!(arr, UInt8Array),
            DataType::UInt16 => ints!(arr, UInt16Array),
            DataType::UInt32 => ints!(arr, UInt32Array),
            DataType::UInt64 => ints!(arr, UInt64Array),
            DataType::Float32 => flts!(arr, Float32Array),
            DataType::Float64 => flts!(arr, Float64Array),
            _ => vec![Value::Null; arr.len()],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_to_arrow_preserves_values_and_type() {
        let a = column_to_arrow(&ColumnData::U32(vec![10, 20, 30]));
        assert_eq!(a.len(), 3);
        let u = a.as_any().downcast_ref::<UInt32Array>().unwrap();
        assert_eq!(u.values(), &[10, 20, 30]);
    }

    #[test]
    fn column_to_arrow_keeps_non_finite_floats_as_values_not_null() {
        let f = column_to_arrow(&ColumnData::F32(vec![0.5, f32::NAN]));
        let fa = f.as_any().downcast_ref::<Float32Array>().unwrap();
        assert_eq!(fa.value(0), 0.5);
        assert!(fa.value(1).is_nan());
        assert_eq!(fa.null_count(), 0);
    }
}
