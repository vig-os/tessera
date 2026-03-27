//! fd5 Merkle tree hashing — direct port of Python's `hash.py`.
//!
//! Implements the content_hash computation:
//! 1. `sorted_attrs_hash(obj)` — SHA-256 of sorted attributes (skip `content_hash`)
//! 2. `dataset_hash(ds)` — `sha256(attrs_hash + sha256(data_bytes))`
//! 3. `group_hash(group)` — `sha256(attrs_hash + child_hashes)` (recursive)
//! 4. `compute_content_hash(file)` — `"sha256:" + sha256(root_group_hash)`
//! 5. `compute_id(inputs)` — `"sha256:" + sha256(sorted_values.join('\0'))`

use sha2::{Digest, Sha256};

use hdf5_metno::types::TypeDescriptor;
use hdf5_metno::{Dataset, File, Group, Location};

use crate::attr_ser::serialize_attr;
use crate::error::Fd5Result;

const CHUNK_HASHES_SUFFIX: &str = "_chunk_hashes";
const EXCLUDED_ATTRS: &[&str] = &["content_hash"];

/// Check if a dataset name is a chunk-hashes auxiliary dataset.
fn is_chunk_hashes_dataset(name: &str) -> bool {
    name.ends_with(CHUNK_HASHES_SUFFIX)
}

/// Compute `sha256(sha256(key + serialize(val)) for key in sorted(attrs))`.
///
/// Exactly matches Python's `_sorted_attrs_hash`.
fn sorted_attrs_hash(obj: &Location) -> Fd5Result<String> {
    let mut h = Sha256::new();

    let mut attr_names = obj.attr_names()?;
    attr_names.sort();

    for key in &attr_names {
        if EXCLUDED_ATTRS.contains(&key.as_str()) {
            continue;
        }
        let attr = obj.attr(key)?;
        let val_bytes = serialize_attr(&attr)?;

        // inner = sha256(key_utf8 + value_bytes)
        let mut inner = Sha256::new();
        inner.update(key.as_bytes());
        inner.update(&val_bytes);
        let inner_hex = format!("{:x}", inner.finalize());

        // Feed hex digest string into outer hasher
        h.update(inner_hex.as_bytes());
    }

    Ok(format!("{:x}", h.finalize()))
}

/// Hash a dataset: `sha256(attrs_hash + sha256(data.tobytes()))`.
///
/// Reads the entire dataset as contiguous row-major bytes.
fn dataset_hash(ds: &Dataset) -> Fd5Result<String> {
    let attrs_h = sorted_attrs_hash(ds)?;

    // Read dataset data as raw bytes
    let data_bytes = read_dataset_bytes(ds)?;
    let data_hash = format!("{:x}", Sha256::digest(&data_bytes));

    let combined = format!("{}{}", attrs_h, data_hash);
    Ok(format!("{:x}", Sha256::digest(combined.as_bytes())))
}

/// Recursively compute the Merkle hash of a group.
///
/// `sha256(sorted_attrs_hash + child_hashes)` where children are
/// processed in sorted key order, `_chunk_hashes` datasets and
/// external links are excluded.
fn group_hash(group: &Group) -> Fd5Result<String> {
    let mut h = Sha256::new();
    h.update(sorted_attrs_hash(group)?.as_bytes());

    let mut member_names = group.member_names()?;
    member_names.sort();

    for key in &member_names {
        if is_chunk_hashes_dataset(key) {
            continue;
        }

        // Check link type — skip external links
        if is_external_link(group, key) {
            continue;
        }

        // Try as group first, then dataset
        if let Ok(child_group) = group.group(key) {
            h.update(group_hash(&child_group)?.as_bytes());
        } else if let Ok(child_ds) = group.dataset(key) {
            h.update(dataset_hash(&child_ds)?.as_bytes());
        }
        // If neither, skip (broken link)
    }

    Ok(format!("{:x}", h.finalize()))
}

/// Check if a member is an external link using iter_visit.
fn is_external_link(group: &Group, name: &str) -> bool {
    use hdf5_metno::LinkType;
    use std::cell::Cell;

    let is_external = Cell::new(false);
    let _ = group.iter_visit_default((), |_group, link_name, link_info, _| {
        if link_name == name && link_info.link_type == LinkType::External {
            is_external.set(true);
            return false; // stop iteration
        }
        true // continue
    });
    is_external.get()
}

/// Read all data from a dataset as contiguous row-major bytes.
///
/// Matches Python's `ds[...].tobytes()`. Uses `read_raw` to handle
/// datasets of any dimensionality.
fn read_dataset_bytes(ds: &Dataset) -> Fd5Result<Vec<u8>> {
    let td = ds.dtype()?.to_descriptor()?;
    let total_elems: usize = ds.shape().iter().product();

    if total_elems == 0 {
        return Ok(Vec::new());
    }

    let bytes = match td {
        TypeDescriptor::Float(hdf5_metno::types::FloatSize::U4) => {
            let data = ds.read_raw::<f32>()?;
            data.iter().flat_map(|x| x.to_ne_bytes()).collect()
        }
        TypeDescriptor::Float(hdf5_metno::types::FloatSize::U8) => {
            let data = ds.read_raw::<f64>()?;
            data.iter().flat_map(|x| x.to_ne_bytes()).collect()
        }
        TypeDescriptor::Integer(int_size) => match int_size {
            hdf5_metno::types::IntSize::U1 => {
                let data = ds.read_raw::<i8>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            hdf5_metno::types::IntSize::U2 => {
                let data = ds.read_raw::<i16>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            hdf5_metno::types::IntSize::U4 => {
                let data = ds.read_raw::<i32>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            hdf5_metno::types::IntSize::U8 => {
                let data = ds.read_raw::<i64>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
        },
        TypeDescriptor::Unsigned(int_size) => match int_size {
            hdf5_metno::types::IntSize::U1 => {
                let data = ds.read_raw::<u8>()?;
                data
            }
            hdf5_metno::types::IntSize::U2 => {
                let data = ds.read_raw::<u16>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            hdf5_metno::types::IntSize::U4 => {
                let data = ds.read_raw::<u32>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
            hdf5_metno::types::IntSize::U8 => {
                let data = ds.read_raw::<u64>()?;
                data.iter().flat_map(|x| x.to_ne_bytes()).collect()
            }
        },
        TypeDescriptor::Boolean => {
            let data = ds.read_raw::<bool>()?;
            data.iter().map(|&b| b as u8).collect()
        }
        // Compound datasets (e.g. event tables) and other types:
        // Read raw bytes using H5Dread with the file's native type.
        _ => {
            read_dataset_raw_bytes(ds, total_elems)?
        }
    };

    Ok(bytes)
}

/// Read raw bytes from a dataset using the file's native type.
///
/// This handles compound types and any other type where we can't use
/// a typed `read_raw<T>()` call. Uses the HDF5 C API directly.
fn read_dataset_raw_bytes(ds: &Dataset, total_elems: usize) -> Fd5Result<Vec<u8>> {
    use hdf5_metno_sys::h5d::{H5Dget_type, H5Dread};
    use hdf5_metno_sys::h5p::H5P_DEFAULT;
    use hdf5_metno_sys::h5s::H5S_ALL;
    use hdf5_metno_sys::h5t::H5Tclose;

    let elem_size = ds.dtype()?.size();
    let total_bytes = total_elems * elem_size;

    // Get the dataset's file type (not a converted one)
    let file_type_id = unsafe { H5Dget_type(ds.id()) };
    if file_type_id < 0 {
        return Err(crate::error::Fd5Error::Other(
            "H5Dget_type failed".to_string(),
        ));
    }

    let mut buf = vec![0u8; total_bytes];
    let ret = unsafe {
        H5Dread(
            ds.id(),
            file_type_id,
            H5S_ALL,
            H5S_ALL,
            H5P_DEFAULT,
            buf.as_mut_ptr().cast(),
        )
    };

    // Close the type we opened
    unsafe { H5Tclose(file_type_id) };

    if ret < 0 {
        return Err(crate::error::Fd5Error::Other(
            "H5Dread failed for compound/opaque dataset".to_string(),
        ));
    }
    Ok(buf)
}

/// Compute the algorithm-prefixed content hash: `"sha256:<hex>"`.
///
/// Direct equivalent of Python's `compute_content_hash(root)`.
pub fn compute_content_hash(file: &File) -> Fd5Result<String> {
    let root = file.as_group()?;
    let root_h = group_hash(&root)?;
    let final_hash = format!("{:x}", Sha256::digest(root_h.as_bytes()));
    Ok(format!("sha256:{}", final_hash))
}

/// Compute the algorithm-prefixed content hash from a Group.
pub fn compute_content_hash_from_group(group: &Group) -> Fd5Result<String> {
    let root_h = group_hash(group)?;
    let final_hash = format!("{:x}", Sha256::digest(root_h.as_bytes()));
    Ok(format!("sha256:{}", final_hash))
}

/// Compute `"sha256:" + sha256(sorted_values.join('\0'))`.
///
/// Direct equivalent of Python's `compute_id(inputs, id_inputs_desc)`.
pub fn compute_id(inputs: &std::collections::BTreeMap<String, String>) -> String {
    let payload: String = inputs
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join("\0");
    let digest = format!("{:x}", Sha256::digest(payload.as_bytes()));
    format!("sha256:{}", digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_compute_id_deterministic() {
        let mut inputs = BTreeMap::new();
        inputs.insert("b".to_string(), "val_b".to_string());
        inputs.insert("a".to_string(), "val_a".to_string());

        let id1 = compute_id(&inputs);
        let id2 = compute_id(&inputs);
        assert_eq!(id1, id2);
        assert!(id1.starts_with("sha256:"));
    }

    #[test]
    fn test_compute_id_sorted_order() {
        // BTreeMap is already sorted, but verify the output matches
        // sha256("val_a\0val_b")
        let mut inputs = BTreeMap::new();
        inputs.insert("a".to_string(), "val_a".to_string());
        inputs.insert("b".to_string(), "val_b".to_string());

        let expected_payload = "val_a\0val_b";
        let expected = format!(
            "sha256:{:x}",
            Sha256::digest(expected_payload.as_bytes())
        );
        assert_eq!(compute_id(&inputs), expected);
    }
}
