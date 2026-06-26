//! Python bindings for Tessera (#210, the v0.5 milestone's first step: `pyo3 → C-ABI → WASM`).
//!
//! Exposes the **read + verify** path — the same surface the conformance reader needs — so a Python
//! caller can open a sealed `.tsra`, read its canonical manifest, pull block bytes, and verify the
//! three-hash seal + per-block digests (identical semantics to `tessera verify`). Built as an abi3
//! extension module: one `tessera.so` works on any CPython ≥ 3.9, with no libpython link.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::path::PathBuf;
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::ProductBuilder;
use tessera_io::{ArrayData, ColumnData, TableData};

create_exception!(
    tessera,
    TesseraError,
    PyException,
    "Raised on any Tessera format / integrity error (bad magic, broken seal, digest mismatch)."
);

fn err<E: std::fmt::Display>(e: E) -> PyErr {
    TesseraError::new_err(e.to_string())
}

/// A handle to a sealed `.tsra` archive. Opening verifies the magic + manifest seal; reading a block
/// verifies its bytes against the recorded digest.
#[pyclass(module = "tessera")]
struct Reader {
    inner: tessera_io::Reader<std::fs::File>,
}

#[pymethods]
impl Reader {
    /// Open and seal-verify a `.tsra` at `path`.
    #[new]
    fn new(path: PathBuf) -> PyResult<Self> {
        Ok(Self {
            inner: tessera_io::Reader::open(&path).map_err(err)?,
        })
    }

    /// The product type declared in the manifest (e.g. `"recon"`, `"listmode"`).
    #[getter]
    fn product(&self) -> String {
        self.inner.manifest().product.clone()
    }

    /// The product's content id (`blake3(JCS(id_inputs))`).
    #[getter]
    fn id(&self) -> String {
        self.inner.manifest().id.clone()
    }

    /// The canonical manifest as a JSON string (parse with `json.loads`).
    fn manifest_json(&self) -> PyResult<String> {
        serde_json::to_string(self.inner.manifest()).map_err(err)
    }

    /// The block names present in the archive.
    fn block_names(&self) -> Vec<String> {
        self.inner.block_names()
    }

    /// The raw encoded bytes of block `name` (digest-verified on read).
    fn read_block<'py>(&mut self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self.inner.read_block(name).map_err(err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Decode an **array** block to `(le_bytes, shape, numpy_code)`. Reconstruct in Python with
    /// `numpy.frombuffer(le_bytes, "<" + numpy_code).reshape(shape)` (C-order, little-endian).
    fn read_array<'py>(
        &mut self,
        py: Python<'py>,
        name: &str,
    ) -> PyResult<(Bound<'py, PyBytes>, Vec<u64>, String)> {
        let spec = self.array_spec(name)?;
        let blob = self.inner.read_block(name).map_err(err)?;
        let data = tessera_io::array::decode(&spec, &blob).map_err(err)?;
        Ok((
            PyBytes::new(py, &data.to_le_bytes()),
            spec.shape,
            data.numpy_code().to_string(),
        ))
    }

    /// Decode a **3-D ROI sub-cube** of an array block — only the intersecting chunks are read +
    /// decoded (the chunked-access win: 7–26× faster than a full decode once the volume spans
    /// multiple 64³ chunks). Returns `(le_bytes, shape, numpy_code)` for the requested sub-volume;
    /// reconstruct with `numpy.frombuffer(buf, "<" + code).reshape(shape)`.
    fn read_array_subset<'py>(
        &mut self,
        py: Python<'py>,
        name: &str,
        origin: Vec<u64>,
        shape: Vec<u64>,
    ) -> PyResult<(Bound<'py, PyBytes>, Vec<u64>, String)> {
        let spec = self.array_spec(name)?;
        let blob = self.inner.read_block(name).map_err(err)?;
        let data = tessera_io::array::decode_subset(&spec, &blob, &origin, &shape).map_err(err)?;
        Ok((
            PyBytes::new(py, &data.to_le_bytes()),
            shape,
            data.numpy_code().to_string(),
        ))
    }

    /// Decode a **table** block to an ordered list of `(column_name, le_bytes, numpy_code)`.
    /// Reconstruct each column with `numpy.frombuffer(le_bytes, "<" + numpy_code)`.
    fn read_table<'py>(
        &mut self,
        py: Python<'py>,
        name: &str,
    ) -> PyResult<Vec<(String, Bound<'py, PyBytes>, String)>> {
        let spec = self.table_spec(name)?;
        let blob = self.inner.read_block(name).map_err(err)?;
        let data = tessera_io::table::decode(&spec, &blob).map_err(err)?;
        Ok(data
            .iter()
            .map(|(n, c)| {
                (
                    n.clone(),
                    PyBytes::new(py, &c.to_le_bytes()),
                    c.numpy_code().to_string(),
                )
            })
            .collect())
    }

    /// Decode a **single column** `column` of table block `name` via Vortex projection — reads only
    /// that column's segments, not the whole table. Returns `(le_bytes, numpy_code)`; reconstruct
    /// with `numpy.frombuffer(le_bytes, "<" + numpy_code)`.
    fn read_table_column<'py>(
        &mut self,
        py: Python<'py>,
        name: &str,
        column: &str,
    ) -> PyResult<(Bound<'py, PyBytes>, String)> {
        let spec = self.table_spec(name)?;
        let blob = self.inner.read_block(name).map_err(err)?;
        let c = tessera_io::table::decode_column(&spec, &blob, column).map_err(err)?;
        Ok((
            PyBytes::new(py, &c.to_le_bytes()),
            c.numpy_code().to_string(),
        ))
    }

    /// Full integrity pass: recompute the three-hash seal and every block digest. Raises
    /// `TesseraError` on any mismatch. Mirrors `tessera verify`.
    fn verify(&mut self) -> PyResult<()> {
        self.inner.manifest().verify().map_err(err)?;
        for name in self.inner.block_names() {
            self.inner.read_block(&name).map_err(err)?;
        }
        Ok(())
    }

    fn __repr__(&self) -> String {
        let m = self.inner.manifest();
        format!(
            "<tessera.Reader product={:?} name={:?} blocks={}>",
            m.product,
            m.name,
            self.inner.block_names().len()
        )
    }
}

impl Reader {
    fn blockref(&self, name: &str) -> PyResult<&BlockRef> {
        self.inner
            .manifest()
            .blocks
            .iter()
            .find(|b| b.name == name)
            .ok_or_else(|| TesseraError::new_err(format!("no block named '{name}'")))
    }

    fn array_spec(&self, name: &str) -> PyResult<ArraySpec> {
        let br = self.blockref(name)?;
        if !matches!(br.kind, BlockKind::Array) {
            return Err(TesseraError::new_err(format!(
                "block '{name}' is not an array (use read_table / read_block)"
            )));
        }
        serde_json::from_value(br.spec.clone()).map_err(err)
    }

    fn table_spec(&self, name: &str) -> PyResult<TableSpec> {
        let br = self.blockref(name)?;
        if !matches!(br.kind, BlockKind::Table) {
            return Err(TesseraError::new_err(format!(
                "block '{name}' is not a table (use read_array / read_block)"
            )));
        }
        serde_json::from_value(br.spec.clone()).map_err(err)
    }
}

/// Builds a sealed `.tsra` from Python: add array/table blocks (from numpy buffers) + metadata +
/// provenance, then `pack(path)`. The mirror of [`Reader`] — together they make `tessera` a
/// round-trippable read+write library.
#[pyclass(module = "tessera")]
struct Builder {
    inner: Option<ProductBuilder>,
    payloads: Vec<tessera_io::BlockPayload>,
}

#[pymethods]
impl Builder {
    /// Start a product: `product` type, `name`, `description`, ISO-8601 `timestamp`.
    #[new]
    fn new(product: String, name: String, description: String, timestamp: String) -> Self {
        Builder {
            inner: Some(ProductBuilder::new(product, name, description, timestamp)),
            payloads: Vec::new(),
        }
    }

    /// Add an N-D **array** block from a little-endian C-order buffer (e.g.
    /// `arr.astype(arr.dtype.newbyteorder("<")).tobytes()`), its `shape`, and the numpy `code`
    /// (`i2/i4/i8`, `u2/u4/u8`, `f4/f8`).
    fn add_array(&mut self, name: &str, code: &str, shape: Vec<u64>, data: &[u8]) -> PyResult<()> {
        let arr = ArrayData::from_le_bytes(code, data).map_err(err)?;
        let n: u64 = shape.iter().product();
        if arr.len() as u64 != n {
            return Err(TesseraError::new_err(format!(
                "array '{name}': {} elements != shape product {n}",
                arr.len()
            )));
        }
        let mut spec = ArraySpec::new(shape, arr.dtype());
        spec.codec = "pcodec".into(); // the array backend speaks pcodec (ArraySpec defaults to zstd)
        let (block_ref, payload) =
            tessera_io::array::array_block(name, &spec, &arr).map_err(err)?;
        self.builder()?.add_block_ref(block_ref);
        self.payloads.push(payload);
        Ok(())
    }

    /// Add a **table** block from ordered `columns` of `(name, code, le_bytes)`. All columns must
    /// have equal length. `row_index` (optional) names the O(1)-take index column.
    #[pyo3(signature = (name, columns, row_index=None))]
    fn add_table(
        &mut self,
        name: &str,
        columns: Vec<(String, String, Vec<u8>)>,
        row_index: Option<String>,
    ) -> PyResult<()> {
        let mut data: TableData = Vec::with_capacity(columns.len());
        for (col, code, bytes) in &columns {
            data.push((
                col.clone(),
                ColumnData::from_le_bytes(code, bytes).map_err(err)?,
            ));
        }
        let rows = data.first().map(|(_, c)| c.len()).unwrap_or(0) as u64;
        if let Some((bad, c)) = data.iter().find(|(_, c)| c.len() as u64 != rows) {
            return Err(TesseraError::new_err(format!(
                "table '{name}': column '{bad}' has {} rows, expected {rows}",
                c.len()
            )));
        }
        let cols = data
            .iter()
            .map(|(n, c)| Column {
                name: n.clone(),
                dtype: c.numpy_code().into(),
                codec: None,
            })
            .collect();
        let spec = TableSpec {
            columns: cols,
            rows,
            row_index,
        };
        let (block_ref, payload) =
            tessera_io::table::table_block(name, &spec, &data).map_err(err)?;
        self.builder()?.add_block_ref(block_ref);
        self.payloads.push(payload);
        Ok(())
    }

    /// Set a schema metadata field; `value_json` is parsed as JSON.
    fn set_field(&mut self, key: &str, value_json: &str) -> PyResult<()> {
        let value: serde_json::Value = serde_json::from_str(value_json).map_err(err)?;
        self.builder()?.with_field(key, value);
        Ok(())
    }

    /// Record a provenance edge (`relation`, e.g. `"ingested_from"`, → `reference`).
    fn add_source(&mut self, relation: &str, reference: &str) -> PyResult<()> {
        self.builder()?
            .add_source(tessera_core::provenance::Source::new(relation, reference));
        Ok(())
    }

    /// Seal (compute the three hashes) and write the sealed `.tsra` to `path`. Returns the content
    /// `id`. Consumes the builder — a second call raises `TesseraError`.
    fn pack(&mut self, path: PathBuf) -> PyResult<String> {
        let builder = self
            .inner
            .take()
            .ok_or_else(|| TesseraError::new_err("builder already packed"))?;
        let manifest = builder.seal().map_err(err)?;
        tessera_io::pack(&manifest, &self.payloads, &path).map_err(err)?;
        Ok(manifest.id)
    }
}

impl Builder {
    fn builder(&mut self) -> PyResult<&mut ProductBuilder> {
        self.inner
            .as_mut()
            .ok_or_else(|| TesseraError::new_err("builder already packed"))
    }
}

/// Open a sealed `.tsra` (convenience for `Reader(path)`).
#[pyfunction]
fn open(path: PathBuf) -> PyResult<Reader> {
    Reader::new(path)
}

/// Open, seal-verify, and read every block of `path`. Raises `TesseraError` on failure.
#[pyfunction]
fn verify(path: PathBuf) -> PyResult<()> {
    Reader::new(path)?.verify()
}

#[pymodule]
fn tessera(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("TesseraError", m.py().get_type::<TesseraError>())?;
    m.add_class::<Reader>()?;
    m.add_class::<Builder>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(verify, m)?)?;
    Ok(())
}
