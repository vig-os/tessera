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
use tessera_core::block::table::TableSpec;
use tessera_core::block::{BlockKind, BlockRef};

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
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(verify, m)?)?;
    Ok(())
}
