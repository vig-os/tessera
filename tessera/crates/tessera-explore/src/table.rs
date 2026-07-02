//! Derived views over a **table** block — the structured row page the CLI `read` (and later the TUI /
//! serve) render, over the cross-block logical view. Column decode stays in [`tessera_io`]; this
//! module projects, windows, and converts columns to JSON cells, returning a typed [`RowPage`], not
//! text. Row-selection *semantics* (negative/open bounds) stay with the caller via the `window`
//! closure — this module only needs the resolved `[lo, hi)`.

use std::io::{Read, Seek};

use serde_json::Value;
use tessera_core::{Error, Result};
use tessera_io::{ColumnData, Reader};

/// A projected, windowed page of a logical table as structured JSON cells (column-major:
/// `cells[column][row]`), plus the full row count for truncation reporting.
#[derive(Debug, Clone, PartialEq)]
pub struct RowPage {
    /// The projected column names, in output order.
    pub columns: Vec<String>,
    /// Cell values, column-major (`cells[col][row]`); non-finite floats decode to `Value::Null`.
    pub cells: Vec<Vec<Value>>,
    /// Total rows in the (logical) table, before windowing.
    pub total: u64,
    /// Rows in this page — the resolved window's length.
    pub shown: u64,
}

/// Convert a decoded column to per-row JSON values. Floats render via their **native** shortest
/// round-trip `Display` (an `f32` shows `0.01`, not its widened-`f64` expansion); non-finite floats
/// (NaN / ±inf) have no JSON encoding → `Value::Null`.
pub fn col_to_values(col: &ColumnData) -> Vec<Value> {
    fn floats<T: std::fmt::Display + Copy>(v: &[T]) -> Vec<Value> {
        v.iter()
            .map(|x| {
                x.to_string()
                    .parse::<serde_json::Number>()
                    .map_or(Value::Null, Value::Number)
            })
            .collect()
    }
    match col {
        ColumnData::I8(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::I16(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::I32(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::I64(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U8(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U16(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U32(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U64(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::F32(v) => floats(v),
        ColumnData::F64(v) => floats(v),
    }
}

/// Read a projected, windowed page of a logical table (cross-block: a read of `events` spans every
/// `events_NNNN`). `columns` empty = all columns in schema order; `window` receives the total row
/// count and returns the half-open `[lo, hi)` to materialise (the caller owns row-selection
/// semantics). Only the requested columns' segments are decoded.
pub fn read_page<R: Read + Seek>(
    reader: &mut Reader<R>,
    prefix: &str,
    columns: &[String],
    window: impl FnOnce(u64) -> (u64, u64),
) -> Result<RowPage> {
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
    let shown = hi.saturating_sub(lo);
    let lo_us = usize::try_from(lo).map_err(|e| Error::Invalid(e.to_string()))?;
    let hi_us = usize::try_from(hi).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut cells: Vec<Vec<Value>> = Vec::with_capacity(selected.len());
    for name in &selected {
        let col = view.column(reader, name)?;
        cells.push(col_to_values(&col.slice(lo_us, hi_us)));
    }
    Ok(RowPage {
        columns: selected,
        cells,
        total,
        shown,
    })
}
