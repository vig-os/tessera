//! The Arrow side of the bridge: a declared table schema from an `ArrowSchema`, and one pushed batch
//! (an `ArrowArray` of struct type) into the writer's [`TableData`].
//!
//! # A declared schema, not an inferred one
//!
//! `tessera ingest table` infers each column from the whole file (ADR-0056 §2: a column is nullable
//! iff it holds a null somewhere). A stream can't do that: the first batch can't know whether a later
//! one holds a null, and inferring per batch would seal the same logical table differently depending
//! on where the nulls fell. So the table's schema is **declared** at `tsra_table_begin` and every batch
//! is checked against it: a field declared nullable seals as a nullable column whether or not any
//! batch holds a null, and a NULL in a field declared non-nullable is an error.
//!
//! # The type map: the clean lane only
//!
//! | Arrow | dtype |
//! | --- | --- |
//! | `Int8` … `Int64` | `i1` … `i8` |
//! | `UInt8` … `UInt64` | `u1` … `u8` |
//! | `Float32`, `Float64` | `f4`, `f8` |
//! | `Boolean` | `b1` |
//! | `Utf8`, `LargeUtf8` | `str` |
//!
//! These are the types whose values cross bit for bit, so the bridge records no transform. Anything
//! else (timestamps, decimals, dictionaries, lists, structs) is refused with the column's name: the
//! ingest lane's recorded transforms (ADR-0056 §2) are whole-file decisions with provenance records,
//! and which of them a producer API should offer is an open question of the ADR, not of this
//! prototype. Nullable columns are fixed-width numerics only, as in the writer (#330).
//!
//! # Copies
//!
//! The import itself is zero-copy: `from_ffi` wraps the producer's buffers and keeps the producer's
//! `release` callback as their owner. Converting to [`ColumnData`] then copies every value once,
//! because the writer's in-memory form is an owned `Vec` per column. The `ArrowArray` is released
//! before the push returns.

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type, UInt16Type, UInt32Type,
    UInt64Type, UInt8Type,
};
use arrow_array::{Array, ArrayRef, StructArray};
use arrow_schema::{DataType, Field};
use tessera_core::block::table::Column;
use tessera_io::table::{ColumnData, TableData};

use crate::error::{CapiError, CapiResult, TsraStatus};

fn schema_err(message: impl Into<String>) -> CapiError {
    CapiError::new(TsraStatus::Schema, message)
}

/// The tessera dtype code of a clean-lane Arrow type, or `None` for every other type.
fn dtype_of(dt: &DataType) -> Option<&'static str> {
    Some(match dt {
        DataType::Int8 => "i1",
        DataType::Int16 => "i2",
        DataType::Int32 => "i4",
        DataType::Int64 => "i8",
        DataType::UInt8 => "u1",
        DataType::UInt16 => "u2",
        DataType::UInt32 => "u4",
        DataType::UInt64 => "u8",
        DataType::Float32 => "f4",
        DataType::Float64 => "f8",
        DataType::Boolean => "b1",
        DataType::Utf8 | DataType::LargeUtf8 => "str",
        _ => return None,
    })
}

/// One declared column: the Arrow field it was declared from (what every batch must match) and the
/// tessera [`Column`] it seals as.
#[derive(Clone, Debug)]
pub struct DeclaredColumn {
    pub field: Field,
    pub column: Column,
}

/// The declared columns of a table from its top-level struct schema, in order.
pub fn declare(schema: &DataType) -> CapiResult<Vec<DeclaredColumn>> {
    let DataType::Struct(fields) = schema else {
        return Err(schema_err(format!(
            "a table schema must be a struct (format \"+s\"), one child per column; got {schema}"
        )));
    };
    if fields.is_empty() {
        return Err(schema_err("a table schema needs at least one column"));
    }
    fields
        .iter()
        .map(|f| {
            let dtype = dtype_of(f.data_type()).ok_or_else(|| {
                schema_err(format!(
                    "column '{}' has Arrow type {}, which the bridge does not map; supported: \
                     Int8-64, UInt8-64, Float32/64, Boolean, Utf8, LargeUtf8",
                    f.name(),
                    f.data_type()
                ))
            })?;
            let mut column = Column::new(f.name(), dtype);
            if f.is_nullable() {
                if !ColumnData::dtype_is_fixed_width(dtype) || dtype == "b1" {
                    return Err(schema_err(format!(
                        "column '{}' is declared nullable, but a {dtype} column can't be: only \
                         fixed-width numeric columns carry a validity mask (#330)",
                        f.name()
                    )));
                }
                column = column.nullable();
            }
            Ok(DeclaredColumn {
                field: f.as_ref().clone(),
                column,
            })
        })
        .collect()
}

/// Check `batch` against the declared columns and copy it into the writer's [`TableData`].
pub fn to_table_data(declared: &[DeclaredColumn], batch: &StructArray) -> CapiResult<TableData> {
    if batch.null_count() != 0 {
        return Err(schema_err(
            "the batch's top-level struct has NULL rows; a row can't be NULL, only a value in a \
             nullable column",
        ));
    }
    let fields = batch.fields();
    if fields.len() != declared.len() {
        return Err(schema_err(format!(
            "the batch has {} columns, the table declares {}",
            fields.len(),
            declared.len()
        )));
    }
    declared
        .iter()
        .zip(fields.iter().zip(batch.columns()))
        .map(|(d, (field, array))| {
            if field.name() != d.field.name() || field.data_type() != d.field.data_type() {
                return Err(schema_err(format!(
                    "batch column '{}' ({}) does not match declared column '{}' ({})",
                    field.name(),
                    field.data_type(),
                    d.field.name(),
                    d.field.data_type()
                )));
            }
            Ok((d.column.name.clone(), column_data(d, array)?))
        })
        .collect()
}

/// One column's values (one copy), wrapped with its validity when the column is declared nullable.
fn column_data(d: &DeclaredColumn, array: &ArrayRef) -> CapiResult<ColumnData> {
    let name = &d.column.name;
    if !d.column.nullable && array.null_count() != 0 {
        return Err(schema_err(format!(
            "column '{name}' is declared non-nullable but the batch holds {} NULLs",
            array.null_count()
        )));
    }
    let values = match array.data_type() {
        DataType::Int8 => ColumnData::I8(array.as_primitive::<Int8Type>().values().to_vec()),
        DataType::Int16 => ColumnData::I16(array.as_primitive::<Int16Type>().values().to_vec()),
        DataType::Int32 => ColumnData::I32(array.as_primitive::<Int32Type>().values().to_vec()),
        DataType::Int64 => ColumnData::I64(array.as_primitive::<Int64Type>().values().to_vec()),
        DataType::UInt8 => ColumnData::U8(array.as_primitive::<UInt8Type>().values().to_vec()),
        DataType::UInt16 => ColumnData::U16(array.as_primitive::<UInt16Type>().values().to_vec()),
        DataType::UInt32 => ColumnData::U32(array.as_primitive::<UInt32Type>().values().to_vec()),
        DataType::UInt64 => ColumnData::U64(array.as_primitive::<UInt64Type>().values().to_vec()),
        DataType::Float32 => ColumnData::F32(array.as_primitive::<Float32Type>().values().to_vec()),
        DataType::Float64 => ColumnData::F64(array.as_primitive::<Float64Type>().values().to_vec()),
        DataType::Boolean => ColumnData::Bool(array.as_boolean().values().iter().collect()),
        DataType::Utf8 => ColumnData::Utf8(
            array
                .as_string::<i32>()
                .iter()
                .map(|v| v.unwrap_or_default().to_owned())
                .collect(),
        ),
        DataType::LargeUtf8 => ColumnData::Utf8(
            array
                .as_string::<i64>()
                .iter()
                .map(|v| v.unwrap_or_default().to_owned())
                .collect(),
        ),
        other => {
            return Err(schema_err(format!(
                "column '{name}': unmapped Arrow type {other} (declare() admits only mapped types)"
            )))
        }
    };
    if !d.column.nullable {
        return Ok(values);
    }
    let validity = match array.nulls() {
        Some(nulls) => nulls.iter().collect(),
        None => vec![true; array.len()],
    };
    Ok(ColumnData::Nullable {
        values: Box::new(values),
        validity,
    })
}

/// The index of the declared column `name`, which must be a non-nullable integer column: the
/// table's sequence-key column.
pub fn key_column(declared: &[DeclaredColumn], name: &str) -> CapiResult<usize> {
    let (i, d) = declared
        .iter()
        .enumerate()
        .find(|(_, d)| d.column.name == name)
        .ok_or_else(|| schema_err(format!("seq_key_column '{name}' is not a declared column")))?;
    let integer = matches!(
        d.column.dtype.as_str(),
        "i1" | "i2" | "i4" | "i8" | "u1" | "u2" | "u4" | "u8"
    );
    if !integer || d.column.nullable {
        return Err(schema_err(format!(
            "seq_key_column '{name}' must be a non-nullable integer column, not {}{}",
            d.column.dtype,
            if d.column.nullable { " (nullable)" } else { "" }
        )));
    }
    Ok(i)
}

/// Check that every event id in `column` lies in batch `seq`'s block `[seq·b, (seq+1)·b)`.
pub fn check_seq_block(column: &(String, ColumnData), seq: u64, b: u64) -> CapiResult<()> {
    let order = |m: String| CapiError::new(TsraStatus::Order, m);
    let lo = i128::from(seq) * i128::from(b);
    let hi = lo + i128::from(b);
    let values: Box<dyn Iterator<Item = i128> + '_> = match &column.1 {
        ColumnData::I8(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::I16(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::I32(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::I64(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::U8(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::U16(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::U32(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        ColumnData::U64(v) => Box::new(v.iter().map(|&x| i128::from(x))),
        _ => {
            return Err(order(format!(
                "seq_key_column '{}' is not an integer column",
                column.0
            )))
        }
    };
    for v in values {
        if v < lo || v >= hi {
            return Err(order(format!(
                "batch {seq} holds {} = {v}, outside its block [{lo}, {hi}) of {b} events: the \
                 producer's blocks don't match the declared seq_block_events (with Geant4, pin \
                 the block size with SetEventModulo({b}) and push each block as seq = first event / {b})",
                column.0
            )));
        }
    }
    Ok(())
}
