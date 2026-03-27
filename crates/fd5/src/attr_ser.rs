//! Deterministic attribute-to-bytes serialization.
//!
//! Must produce byte-identical output to Python's `_serialize_attr` (hash.py L76-85):
//!
//! - `str`        → `.encode("utf-8")`
//! - `bytes`      → as-is
//! - `np.ndarray` → `.tobytes()` (row-major C-order)
//! - `np.generic` → `np.array(value).tobytes()`
//! - fallback     → `str(value).encode("utf-8")`
//!
//! In HDF5-metno, attributes arrive as typed values. We read raw bytes for
//! numeric types and UTF-8 for strings to match Python exactly.
//!
//! **Important**: Uses `read_raw` (not `read_1d`) for arrays because
//! attributes can be multi-dimensional (e.g. 4×4 affine matrices).

use hdf5_metno::types::{FloatSize, IntSize, TypeDescriptor, VarLenAscii, VarLenUnicode};
use hdf5_metno::Attribute;

use crate::error::Fd5Result;

/// Serialize an HDF5 attribute value to bytes, matching Python's `_serialize_attr`.
pub fn serialize_attr(attr: &Attribute) -> Fd5Result<Vec<u8>> {
    let td = attr.dtype()?.to_descriptor()?;

    if attr.is_scalar() {
        serialize_scalar(attr, &td)
    } else {
        serialize_array(attr, &td)
    }
}

fn serialize_scalar(attr: &Attribute, td: &TypeDescriptor) -> Fd5Result<Vec<u8>> {
    match td {
        // String types → UTF-8 bytes (matching Python str.encode("utf-8"))
        TypeDescriptor::VarLenUnicode => {
            let v: VarLenUnicode = attr.read_scalar()?;
            Ok(v.as_str().as_bytes().to_vec())
        }
        TypeDescriptor::VarLenAscii => {
            let v: VarLenAscii = attr.read_scalar()?;
            Ok(v.as_str().as_bytes().to_vec())
        }
        TypeDescriptor::FixedAscii(_) | TypeDescriptor::FixedUnicode(_) => {
            // Read raw, trim trailing nulls, return UTF-8
            let raw = attr.read_raw::<u8>()?;
            let s = String::from_utf8_lossy(&raw);
            let trimmed = s.trim_end_matches('\0');
            Ok(trimmed.as_bytes().to_vec())
        }

        // Numeric scalars → np.array(value).tobytes()
        TypeDescriptor::Integer(int_size) => Ok(match int_size {
            IntSize::U1 => attr.read_scalar::<i8>()?.to_ne_bytes().to_vec(),
            IntSize::U2 => attr.read_scalar::<i16>()?.to_ne_bytes().to_vec(),
            IntSize::U4 => attr.read_scalar::<i32>()?.to_ne_bytes().to_vec(),
            IntSize::U8 => attr.read_scalar::<i64>()?.to_ne_bytes().to_vec(),
        }),
        TypeDescriptor::Unsigned(int_size) => Ok(match int_size {
            IntSize::U1 => attr.read_scalar::<u8>()?.to_ne_bytes().to_vec(),
            IntSize::U2 => attr.read_scalar::<u16>()?.to_ne_bytes().to_vec(),
            IntSize::U4 => attr.read_scalar::<u32>()?.to_ne_bytes().to_vec(),
            IntSize::U8 => attr.read_scalar::<u64>()?.to_ne_bytes().to_vec(),
        }),
        TypeDescriptor::Float(float_size) => Ok(match float_size {
            FloatSize::U4 => attr.read_scalar::<f32>()?.to_ne_bytes().to_vec(),
            FloatSize::U8 => attr.read_scalar::<f64>()?.to_ne_bytes().to_vec(),
        }),
        TypeDescriptor::Boolean => {
            let v: bool = attr.read_scalar()?;
            Ok(vec![v as u8])
        }

        // Fallback: str(value).encode("utf-8")
        _ => {
            let raw = attr.read_raw::<u8>()?;
            Ok(raw)
        }
    }
}

/// Serialize a non-scalar attribute to bytes.
///
/// Uses `read_raw` to handle any dimensionality (1D, 2D, etc.).
fn serialize_array(attr: &Attribute, td: &TypeDescriptor) -> Fd5Result<Vec<u8>> {
    match td {
        TypeDescriptor::Integer(int_size) => Ok(match int_size {
            IntSize::U1 => {
                let v = attr.read_raw::<i8>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            IntSize::U2 => {
                let v = attr.read_raw::<i16>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            IntSize::U4 => {
                let v = attr.read_raw::<i32>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            IntSize::U8 => {
                let v = attr.read_raw::<i64>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
        }),
        TypeDescriptor::Unsigned(int_size) => Ok(match int_size {
            IntSize::U1 => attr.read_raw::<u8>()?,
            IntSize::U2 => {
                let v = attr.read_raw::<u16>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            IntSize::U4 => {
                let v = attr.read_raw::<u32>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            IntSize::U8 => {
                let v = attr.read_raw::<u64>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
        }),
        TypeDescriptor::Float(float_size) => Ok(match float_size {
            FloatSize::U4 => {
                let v = attr.read_raw::<f32>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            FloatSize::U8 => {
                let v = attr.read_raw::<f64>()?;
                v.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
        }),
        TypeDescriptor::Boolean => {
            let v = attr.read_raw::<bool>()?;
            Ok(v.iter().map(|&b| b as u8).collect())
        }
        // For string arrays in attributes, concatenate UTF-8 bytes
        TypeDescriptor::VarLenUnicode => {
            let v = attr.read_raw::<VarLenUnicode>()?;
            let mut buf = Vec::new();
            for s in &v {
                buf.extend_from_slice(s.as_str().as_bytes());
            }
            Ok(buf)
        }
        TypeDescriptor::VarLenAscii => {
            let v = attr.read_raw::<VarLenAscii>()?;
            let mut buf = Vec::new();
            for s in &v {
                buf.extend_from_slice(s.as_str().as_bytes());
            }
            Ok(buf)
        }
        // Fallback: try reading raw bytes
        _ => Ok(attr.read_raw::<u8>()?),
    }
}
