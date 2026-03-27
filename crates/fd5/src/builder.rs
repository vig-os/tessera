//! fd5 builder — creates sealed HDF5 files with inline Merkle-tree hashing.
//!
//! Mirrors Python's `fd5.create` context-manager API. The builder:
//! 1. Opens a temp HDF5 file and writes root attributes
//! 2. Delegates product-specific writes via `ProductSchema`
//! 3. Seals the file: embeds schema, computes id + content_hash, renames
//!
//! Data hashes are computed inline during `create_dataset` calls (tee pattern)
//! and cached so that `compute_content_hash` can skip re-reading datasets.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use hdf5_metno::types::VarLenUnicode;
use hdf5_metno::{File, Group};
use sha2::{Digest, Sha256};

use crate::error::{Fd5Error, Fd5Result};
use crate::h5io::{dict_to_h5, write_attr_i64, write_attr_str};
use crate::hash::{compute_content_hash, compute_id};
use crate::naming::generate_filename;
use crate::product::{get_schema, ProductSchema};

/// Shared hash caches between `Fd5Builder` and `HashTrackingGroup` instances.
type DataHashCache = Rc<RefCell<HashMap<String, String>>>;
type ChunkDigestCache = Rc<RefCell<HashMap<String, Vec<String>>>>;

// ---------------------------------------------------------------------------
// HashTrackingGroup
// ---------------------------------------------------------------------------

/// Wraps an `hdf5_metno::Group` to compute data hashes inline during writes.
///
/// Cached data hashes (`sha256(data_bytes)`) and per-chunk digests are stored
/// in shared caches keyed by the dataset's absolute HDF5 path.
pub struct HashTrackingGroup {
    group: Group,
    data_hash_cache: DataHashCache,
    chunk_digest_cache: ChunkDigestCache,
}

impl HashTrackingGroup {
    fn new(
        group: Group,
        data_hash_cache: DataHashCache,
        chunk_digest_cache: ChunkDigestCache,
    ) -> Self {
        Self {
            group,
            data_hash_cache,
            chunk_digest_cache,
        }
    }

    /// Create a sub-group, returning a wrapped `HashTrackingGroup`.
    pub fn create_group(&self, name: &str) -> Fd5Result<HashTrackingGroup> {
        let grp = self.group.create_group(name)?;
        Ok(HashTrackingGroup::new(
            grp,
            Rc::clone(&self.data_hash_cache),
            Rc::clone(&self.chunk_digest_cache),
        ))
    }

    /// Create a dataset, write data, and cache the SHA-256 hash of the raw bytes.
    fn create_dataset_and_hash<T: hdf5_metno::types::H5Type>(
        &self,
        name: &str,
        data: &[T],
    ) -> Fd5Result<()> {
        let ds = self
            .group
            .new_dataset::<T>()
            .shape([data.len()])
            .create(name)?;
        ds.write(data)?;

        let byte_len = data.len() * std::mem::size_of::<T>();
        let byte_ptr = data.as_ptr() as *const u8;
        // SAFETY: &[T] is a contiguous, aligned, initialized buffer.
        let bytes = unsafe { std::slice::from_raw_parts(byte_ptr, byte_len) };
        let data_hash = format!("{:x}", Sha256::digest(bytes));

        self.data_hash_cache
            .borrow_mut()
            .insert(ds.name(), data_hash);

        Ok(())
    }

    /// Create a dataset of f64 values, hashing inline.
    pub fn create_dataset_f64(&self, name: &str, data: &[f64]) -> Fd5Result<()> {
        self.create_dataset_and_hash(name, data)
    }

    /// Create a dataset of f32 values, hashing inline.
    pub fn create_dataset_f32(&self, name: &str, data: &[f32]) -> Fd5Result<()> {
        self.create_dataset_and_hash(name, data)
    }

    /// Create a dataset of i64 values, hashing inline.
    pub fn create_dataset_i64(&self, name: &str, data: &[i64]) -> Fd5Result<()> {
        self.create_dataset_and_hash(name, data)
    }

    /// Create a dataset of i32 values, hashing inline.
    pub fn create_dataset_i32(&self, name: &str, data: &[i32]) -> Fd5Result<()> {
        self.create_dataset_and_hash(name, data)
    }

    /// Create a dataset of u8 values, hashing inline.
    pub fn create_dataset_u8(&self, name: &str, data: &[u8]) -> Fd5Result<()> {
        self.create_dataset_and_hash(name, data)
    }

    /// Write a string attribute on this group.
    pub fn write_attr_str(&self, name: &str, value: &str) -> Fd5Result<()> {
        write_attr_str(&self.group, name, value)
    }

    /// Write an i64 attribute on this group.
    pub fn write_attr_i64(&self, name: &str, value: i64) -> Fd5Result<()> {
        write_attr_i64(&self.group, name, value)
    }

    /// Access the underlying HDF5 group (for advanced use).
    pub fn group(&self) -> &Group {
        &self.group
    }
}

// ---------------------------------------------------------------------------
// Fd5Builder
// ---------------------------------------------------------------------------

/// Builder that orchestrates fd5 file creation.
///
/// Do not instantiate directly -- use [`create()`].
pub struct Fd5Builder {
    file: File,
    tmp_path: PathBuf,
    out_dir: PathBuf,
    product_type: String,
    timestamp: String,
    schema: Box<dyn ProductSchema>,
    data_hash_cache: DataHashCache,
    chunk_digest_cache: ChunkDigestCache,
}

impl Fd5Builder {
    /// Write product-specific data through the registered schema.
    pub fn write_product(&self, data: &serde_json::Value) -> Fd5Result<()> {
        let group = self.file.as_group()?;
        let tracking = HashTrackingGroup::new(
            group,
            Rc::clone(&self.data_hash_cache),
            Rc::clone(&self.chunk_digest_cache),
        );
        self.schema.write(&tracking, data)
    }

    /// Write metadata group from a JSON value (nested dict -> HDF5 groups/attrs).
    pub fn write_metadata(&self, metadata: &serde_json::Value) -> Fd5Result<()> {
        let grp = self.file.create_group("metadata")?;
        dict_to_h5(&grp, metadata)
    }

    /// Access the underlying HDF5 file (for advanced writes).
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Seal the file: validate, embed schema, compute id + content_hash, rename.
    ///
    /// Consumes self -- the file cannot be used after sealing.
    pub fn seal(self) -> Fd5Result<PathBuf> {
        self.validate()?;
        self.write_chunk_hashes()?;

        let schema_json = self.schema.json_schema();
        let schema_str = serde_json::to_string(&schema_json)?;
        let root = self.file.as_group()?;

        let vlu: VarLenUnicode = schema_str
            .parse()
            .map_err(|e| Fd5Error::Other(format!("{}", e)))?;
        root.new_attr::<VarLenUnicode>()
            .shape(())
            .create("_schema")?
            .write_scalar(&vlu)?;

        let id_keys = self.schema.id_inputs();
        let mut id_inputs = BTreeMap::new();
        for key in &id_keys {
            let val = read_root_attr_str(&root, key).unwrap_or_default();
            id_inputs.insert(key.clone(), val);
        }
        let file_id = compute_id(&id_inputs);

        write_attr_str(&root, "id", &file_id)?;
        write_attr_str(&root, "id_inputs", &id_keys.join(" + "))?;

        // content_hash is computed from the file directly (data already written)
        let content_hash = compute_content_hash(&self.file)?;
        write_attr_str(&root, "content_hash", &content_hash)?;

        self.file.flush()?;
        drop(root);
        self.file.close()?;

        let product_slug = self.product_type.replace('/', "-");
        let filename = generate_filename(&product_slug, &file_id, Some(&self.timestamp));
        let final_path = self.out_dir.join(filename);
        std::fs::rename(&self.tmp_path, &final_path)?;

        Ok(final_path)
    }

    fn validate(&self) -> Fd5Result<()> {
        let root = self.file.as_group()?;
        for attr_name in &["name", "description", "timestamp"] {
            let val = read_root_attr_str(&root, attr_name).unwrap_or_default();
            if val.is_empty() {
                return Err(Fd5Error::Other(format!(
                    "Required attribute '{}' is missing or empty",
                    attr_name
                )));
            }
        }
        Ok(())
    }

    fn write_chunk_hashes(&self) -> Fd5Result<()> {
        let cache = self.chunk_digest_cache.borrow();
        for (ds_path, digests) in cache.iter() {
            let ds = self.file.dataset(ds_path)?;
            let parent_path = ds_path
                .rsplit_once('/')
                .map(|(p, _)| if p.is_empty() { "/" } else { p })
                .unwrap_or("/");
            let ds_name = ds_path.rsplit_once('/').map(|(_, n)| n).unwrap_or(ds_path);
            let hashes_name = format!("{}_chunk_hashes", ds_name);

            let parent = if parent_path == "/" {
                self.file.as_group()?
            } else {
                self.file.group(parent_path)?
            };

            let vlu_digests: Vec<VarLenUnicode> = digests
                .iter()
                .map(|d| d.parse::<VarLenUnicode>().unwrap())
                .collect();
            let chunk_ds = parent
                .new_dataset::<VarLenUnicode>()
                .shape([vlu_digests.len()])
                .create(hashes_name.as_str())?;
            chunk_ds.write(&vlu_digests)?;

            write_attr_str(&chunk_ds, "algorithm", "sha256")?;
            drop(ds);
        }
        Ok(())
    }
}

/// Read a string attribute from a group, returning `None` if not found.
fn read_root_attr_str(group: &Group, name: &str) -> Option<String> {
    group
        .attr(name)
        .ok()
        .and_then(|a| {
            a.read_scalar::<VarLenUnicode>()
                .map(|v| v.as_str().to_string())
                .ok()
                .or_else(|| {
                    a.read_scalar::<hdf5_metno::types::VarLenAscii>()
                        .map(|v| v.as_str().to_string())
                        .ok()
                })
        })
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Create a new fd5 builder -- analogous to Python's `fd5.create()` context manager.
///
/// Opens a temporary HDF5 file, writes root attributes, and returns a builder
/// that can be used to write product data and metadata. Call `seal()` to finalize.
///
/// # Errors
///
/// Returns an error if the product schema is not registered or if the temp file
/// cannot be created.
pub fn create(
    out_dir: &Path,
    product: &str,
    name: &str,
    description: &str,
    timestamp: &str,
) -> Fd5Result<Fd5Builder> {
    let schema = get_schema(product)?;

    std::fs::create_dir_all(out_dir)?;

    let product_slug = product.replace('/', "_");
    let tmp_name = format!(".fd5_{}.h5.tmp", product_slug);
    let tmp_path = out_dir.join(tmp_name);
    let file = File::create(&tmp_path)?;

    let root = file.as_group()?;
    write_attr_str(&root, "product", product)?;
    write_attr_str(&root, "name", name)?;
    write_attr_str(&root, "description", description)?;
    write_attr_str(&root, "timestamp", timestamp)?;
    write_attr_i64(&root, "_schema_version", 1)?;

    let data_hash_cache: DataHashCache = Rc::new(RefCell::new(HashMap::new()));
    let chunk_digest_cache: ChunkDigestCache = Rc::new(RefCell::new(HashMap::new()));

    Ok(Fd5Builder {
        file,
        tmp_path,
        out_dir: out_dir.to_path_buf(),
        product_type: product.to_string(),
        timestamp: timestamp.to_string(),
        schema,
        data_hash_cache,
        chunk_digest_cache,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::{register_schema, TestProductSchema};
    use crate::verify;
    use tempfile::TempDir;

    fn register_test_schema() {
        register_schema(Box::new(TestProductSchema));
    }

    #[test]
    fn test_create_and_seal() {
        register_test_schema();
        let tmp_dir = TempDir::new().unwrap();

        let builder = create(
            tmp_dir.path(),
            "test/product",
            "my-test",
            "A test file",
            "2024-01-15T10:30:00",
        )
        .unwrap();

        let data = serde_json::json!({"values": [1.0, 2.0, 3.0]});
        builder.write_product(&data).unwrap();

        let sealed_path = builder.seal().unwrap();
        assert!(sealed_path.exists());
        assert!(sealed_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".h5"));

        // Verify with the existing verify module
        let status = verify::verify(&sealed_path).unwrap();
        match status {
            verify::Fd5Status::Valid(_) => {} // expected
            other => panic!("Expected Valid, got {:?}", other),
        }
    }

    #[test]
    fn test_content_hash_deterministic() {
        register_test_schema();
        let tmp_dir = TempDir::new().unwrap();

        let make_file = |subdir: &str| -> String {
            // Each call needs its own schema registration since get_schema removes it
            register_test_schema();
            let out = tmp_dir.path().join(subdir);
            let builder = create(
                &out,
                "test/product",
                "my-test",
                "A test file",
                "2024-01-15T10:30:00",
            )
            .unwrap();

            let data = serde_json::json!({"values": [1.0, 2.0, 3.0]});
            builder.write_product(&data).unwrap();
            let path = builder.seal().unwrap();

            // Read content_hash from sealed file
            let file = File::open(&path).unwrap();
            let group = file.as_group().unwrap();
            read_root_attr_str(&group, "content_hash").unwrap()
        };

        let hash1 = make_file("a");
        let hash2 = make_file("b");
        assert_eq!(hash1, hash2, "content_hash should be deterministic");
    }

    #[test]
    fn test_missing_required_attr_fails() {
        register_test_schema();
        let tmp_dir = TempDir::new().unwrap();

        let builder = create(
            tmp_dir.path(),
            "test/product",
            "", // empty name
            "A test file",
            "2024-01-15T10:30:00",
        )
        .unwrap();

        let result = builder.seal();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("name"),
            "Error should mention 'name': {}",
            err_msg
        );
    }

    #[test]
    fn test_write_metadata() {
        register_test_schema();
        let tmp_dir = TempDir::new().unwrap();

        let builder = create(
            tmp_dir.path(),
            "test/product",
            "my-test",
            "A test file",
            "2024-01-15T10:30:00",
        )
        .unwrap();

        let metadata = serde_json::json!({
            "subject": "test-subject-01",
            "scanner": {
                "model": "Explorer",
                "manufacturer": "United Imaging"
            }
        });
        builder.write_metadata(&metadata).unwrap();

        let data = serde_json::json!({"values": [1.0, 2.0, 3.0]});
        builder.write_product(&data).unwrap();

        let sealed_path = builder.seal().unwrap();
        assert!(sealed_path.exists());

        // Verify the sealed file
        let status = verify::verify(&sealed_path).unwrap();
        match status {
            verify::Fd5Status::Valid(_) => {}
            other => panic!("Expected Valid, got {:?}", other),
        }
    }
}
