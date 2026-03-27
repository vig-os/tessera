//! Lossless round-trip between `serde_json::Value` trees and HDF5 groups/attrs.
//!
//! Type mapping follows the Python `fd5.h5io` module:
//! - JSON objects become HDF5 sub-groups
//! - JSON strings become VarLenUnicode attributes
//! - JSON numbers become i64 or f64 attributes
//! - JSON booleans become bool attributes
//! - JSON arrays become array attributes (typed by first element)
//! - JSON null values are skipped (absence encodes null)

use hdf5_metno::types::VarLenUnicode;
use hdf5_metno::{Group, Location};
use serde_json::Value;

use crate::error::{Fd5Error, Fd5Result};

/// Write a `serde_json::Value` tree to an HDF5 group.
///
/// Objects become subgroups, scalars become attributes.
/// Keys are written in sorted order for deterministic layout.
pub fn dict_to_h5(group: &Group, data: &Value) -> Fd5Result<()> {
    let obj = data
        .as_object()
        .ok_or_else(|| Fd5Error::Other("dict_to_h5 expects a JSON object".into()))?;

    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();

    for key in keys {
        let value = &obj[key];
        if value.is_null() {
            continue;
        }
        write_value(group, key, value)?;
    }
    Ok(())
}

/// Read an HDF5 group tree into a `serde_json::Value`.
///
/// Reads attributes and sub-groups; datasets are not read.
pub fn h5_to_dict(group: &Group) -> Fd5Result<Value> {
    let mut map = serde_json::Map::new();

    let mut attr_names = group.attr_names()?;
    attr_names.sort();
    for key in &attr_names {
        let attr = group.attr(key)?;
        let val = read_attr_value(&attr)?;
        map.insert(key.clone(), val);
    }

    let mut member_names = group.member_names()?;
    member_names.sort();
    for key in &member_names {
        if let Ok(child_group) = group.group(key) {
            let child_val = h5_to_dict(&child_group)?;
            map.insert(key.clone(), child_val);
        }
    }

    Ok(Value::Object(map))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn write_value(group: &Group, key: &str, value: &Value) -> Fd5Result<()> {
    match value {
        Value::Object(_) => {
            let sub = group.create_group(key)?;
            dict_to_h5(&sub, value)?;
        }
        Value::Bool(b) => {
            write_attr_bool(group, key, *b)?;
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                write_attr_i64(group, key, i)?;
            } else if let Some(f) = n.as_f64() {
                write_attr_f64(group, key, f)?;
            }
        }
        Value::String(s) => {
            write_attr_str(group, key, s)?;
        }
        Value::Array(arr) => {
            write_list(group, key, arr)?;
        }
        Value::Null => {}
    }
    Ok(())
}

fn write_list(group: &Group, key: &str, arr: &[Value]) -> Fd5Result<()> {
    if arr.is_empty() {
        let data: Vec<f64> = vec![];
        group
            .new_attr::<f64>()
            .shape([0])
            .create(key)?
            .write_raw(&data)?;
        return Ok(());
    }

    let first = &arr[0];
    if first.is_boolean() {
        let bools: Vec<bool> = arr
            .iter()
            .map(|v| v.as_bool().unwrap_or(false))
            .collect();
        group
            .new_attr::<bool>()
            .shape([bools.len()])
            .create(key)?
            .write_raw(&bools)?;
    } else if first.is_number() {
        if first.is_i64() && arr.iter().all(|v| v.is_i64()) {
            let ints: Vec<i64> = arr
                .iter()
                .map(|v| v.as_i64().unwrap_or(0))
                .collect();
            group
                .new_attr::<i64>()
                .shape([ints.len()])
                .create(key)?
                .write_raw(&ints)?;
        } else {
            let floats: Vec<f64> = arr
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0))
                .collect();
            group
                .new_attr::<f64>()
                .shape([floats.len()])
                .create(key)?
                .write_raw(&floats)?;
        }
    } else if first.is_string() {
        let strings: Vec<VarLenUnicode> = arr
            .iter()
            .map(|v| {
                let s = v.as_str().unwrap_or("");
                s.parse::<VarLenUnicode>().expect("valid unicode string")
            })
            .collect();
        group
            .new_attr::<VarLenUnicode>()
            .shape([strings.len()])
            .create(key)?
            .write_raw(&strings)?;
    } else {
        return Err(Fd5Error::Other(format!(
            "Unsupported array element type for key '{}'",
            key
        )));
    }
    Ok(())
}

/// Write a VarLenUnicode string attribute.
pub fn write_attr_str(loc: &Location, name: &str, value: &str) -> Fd5Result<()> {
    let vlu: VarLenUnicode = value.parse().map_err(|e| {
        Fd5Error::Other(format!("Invalid string for attribute '{}': {}", name, e))
    })?;
    loc.new_attr::<VarLenUnicode>()
        .shape(())
        .create(name)?
        .write_scalar(&vlu)?;
    Ok(())
}

/// Write an i64 attribute.
pub fn write_attr_i64(loc: &Location, name: &str, value: i64) -> Fd5Result<()> {
    loc.new_attr::<i64>()
        .shape(())
        .create(name)?
        .write_scalar(&value)?;
    Ok(())
}

/// Write an f64 attribute.
pub fn write_attr_f64(loc: &Location, name: &str, value: f64) -> Fd5Result<()> {
    loc.new_attr::<f64>()
        .shape(())
        .create(name)?
        .write_scalar(&value)?;
    Ok(())
}

/// Write a bool attribute.
pub fn write_attr_bool(loc: &Location, name: &str, value: bool) -> Fd5Result<()> {
    loc.new_attr::<bool>()
        .shape(())
        .create(name)?
        .write_scalar(&value)?;
    Ok(())
}

/// Read an HDF5 attribute into a `serde_json::Value`.
fn read_attr_value(attr: &hdf5_metno::Attribute) -> Fd5Result<Value> {
    use hdf5_metno::types::{FloatSize, IntSize, TypeDescriptor};

    let td = attr.dtype()?.to_descriptor()?;

    if attr.is_scalar() {
        match &td {
            TypeDescriptor::VarLenUnicode => {
                let v: VarLenUnicode = attr.read_scalar()?;
                Ok(Value::String(v.as_str().to_string()))
            }
            TypeDescriptor::VarLenAscii => {
                let v: hdf5_metno::types::VarLenAscii = attr.read_scalar()?;
                Ok(Value::String(v.as_str().to_string()))
            }
            TypeDescriptor::Integer(int_size) => {
                let val: i64 = match int_size {
                    IntSize::U1 => attr.read_scalar::<i8>()? as i64,
                    IntSize::U2 => attr.read_scalar::<i16>()? as i64,
                    IntSize::U4 => attr.read_scalar::<i32>()? as i64,
                    IntSize::U8 => attr.read_scalar::<i64>()?,
                };
                Ok(Value::Number(serde_json::Number::from(val)))
            }
            TypeDescriptor::Unsigned(int_size) => {
                let val: u64 = match int_size {
                    IntSize::U1 => attr.read_scalar::<u8>()? as u64,
                    IntSize::U2 => attr.read_scalar::<u16>()? as u64,
                    IntSize::U4 => attr.read_scalar::<u32>()? as u64,
                    IntSize::U8 => attr.read_scalar::<u64>()?,
                };
                Ok(Value::Number(serde_json::Number::from(val)))
            }
            TypeDescriptor::Float(float_size) => {
                let val: f64 = match float_size {
                    FloatSize::U4 => attr.read_scalar::<f32>()? as f64,
                    FloatSize::U8 => attr.read_scalar::<f64>()?,
                };
                Ok(serde_json::json!(val))
            }
            TypeDescriptor::Boolean => {
                let v: bool = attr.read_scalar()?;
                Ok(Value::Bool(v))
            }
            _ => {
                let raw = attr.read_raw::<u8>()?;
                Ok(Value::String(String::from_utf8_lossy(&raw).to_string()))
            }
        }
    } else {
        // Array attribute
        read_array_attr_value(attr, &td)
    }
}

fn read_array_attr_value(
    attr: &hdf5_metno::Attribute,
    td: &hdf5_metno::types::TypeDescriptor,
) -> Fd5Result<Value> {
    use hdf5_metno::types::{FloatSize, IntSize, TypeDescriptor};

    match td {
        TypeDescriptor::VarLenUnicode => {
            let v = attr.read_raw::<VarLenUnicode>()?;
            let arr: Vec<Value> = v.iter().map(|s| Value::String(s.as_str().to_string())).collect();
            Ok(Value::Array(arr))
        }
        TypeDescriptor::VarLenAscii => {
            let v = attr.read_raw::<hdf5_metno::types::VarLenAscii>()?;
            let arr: Vec<Value> = v.iter().map(|s| Value::String(s.as_str().to_string())).collect();
            Ok(Value::Array(arr))
        }
        TypeDescriptor::Integer(int_size) => {
            let vals: Vec<i64> = match int_size {
                IntSize::U1 => attr.read_raw::<i8>()?.iter().map(|&v| v as i64).collect(),
                IntSize::U2 => attr.read_raw::<i16>()?.iter().map(|&v| v as i64).collect(),
                IntSize::U4 => attr.read_raw::<i32>()?.iter().map(|&v| v as i64).collect(),
                IntSize::U8 => attr.read_raw::<i64>()?.to_vec(),
            };
            let arr: Vec<Value> = vals.into_iter().map(|v| Value::Number(v.into())).collect();
            Ok(Value::Array(arr))
        }
        TypeDescriptor::Float(float_size) => {
            let vals: Vec<f64> = match float_size {
                FloatSize::U4 => attr.read_raw::<f32>()?.iter().map(|&v| v as f64).collect(),
                FloatSize::U8 => attr.read_raw::<f64>()?.to_vec(),
            };
            let arr: Vec<Value> = vals.into_iter().map(|v| serde_json::json!(v)).collect();
            Ok(Value::Array(arr))
        }
        TypeDescriptor::Boolean => {
            let v = attr.read_raw::<bool>()?;
            let arr: Vec<Value> = v.iter().map(|&b| Value::Bool(b)).collect();
            Ok(Value::Array(arr))
        }
        _ => {
            let raw = attr.read_raw::<u8>()?;
            Ok(Value::String(String::from_utf8_lossy(&raw).to_string()))
        }
    }
}
