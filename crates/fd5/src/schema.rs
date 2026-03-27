//! JSON Schema loading, validation, and embedded `_schema` extraction.
//!
//! Mirrors Python's `schema.py`.

use hdf5_metno::File;
use serde_json::Value;

use crate::error::{Fd5Error, Fd5Result};

/// Extract and parse the `_schema` JSON attribute from an fd5 file.
pub fn dump_schema(file: &File) -> Fd5Result<Value> {
    let group = file.as_group()?;
    let attr = group.attr("_schema").map_err(|_| {
        Fd5Error::MissingAttribute("_schema".to_string())
    })?;
    let raw: String = attr.read_scalar::<hdf5_metno::types::VarLenUnicode>()
        .map(|v| v.as_str().to_string())
        .or_else(|_| attr.read_scalar::<hdf5_metno::types::VarLenAscii>().map(|v| v.as_str().to_string()))
        .map_err(|e| Fd5Error::Other(format!("Failed to read _schema attribute: {e}")))?;
    let schema: Value = serde_json::from_str(&raw)?;
    Ok(schema)
}

/// Read the `_schema_version` attribute (int64).
pub fn schema_version(file: &File) -> Fd5Result<i64> {
    let group = file.as_group()?;
    let attr = group.attr("_schema_version").map_err(|_| {
        Fd5Error::MissingAttribute("_schema_version".to_string())
    })?;
    let v: i64 = attr.read_scalar()?;
    Ok(v)
}

/// Check if an fd5 file has an embedded schema.
pub fn has_schema(file: &File) -> bool {
    file.as_group()
        .ok()
        .and_then(|g| g.attr("_schema").ok())
        .is_some()
}
