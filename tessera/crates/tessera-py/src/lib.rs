//! Python bindings for Tessera (#210, the v0.5 milestone's first step: `pyo3 → C-ABI → WASM`).
//!
//! Exposes the **read + verify** path — the same surface the conformance reader needs — so a Python
//! caller can open a sealed `.tsra`, read its canonical manifest, pull block bytes, and verify the
//! three-hash seal + per-block digests (identical semantics to `tessera verify`). Built as an abi3
//! extension module: one `tessera.so` works on any CPython ≥ 3.9, with no libpython link.

use pyo3::create_exception;
use pyo3::exceptions::{
    PyException, PyRecursionError, PyTypeError, PyUnicodeEncodeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use std::collections::{BTreeMap, HashSet};
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

    /// Decode a **single column** of the *logical* multi-block table named by `prefix`
    /// (`events` / `events_NNNN`) — the same surface as [`Self::read_table_column`] but spanning
    /// every block in manifest order. Each per-block read uses Vortex column projection (only the
    /// requested column's segments), then the per-block columns are concatenated. Returns
    /// `(le_bytes, numpy_code)`; reconstruct with `numpy.frombuffer(le_bytes, "<" + numpy_code)`.
    fn read_logical_table_column<'py>(
        &mut self,
        py: Python<'py>,
        prefix: &str,
        column: &str,
    ) -> PyResult<(Bound<'py, PyBytes>, String)> {
        let view = self.inner.logical_table(prefix).map_err(err)?;
        let c = view.column(&mut self.inner, column).map_err(err)?;
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
    /// (`i1/i2/i4/i8`, `u1/u2/u4/u8`, `f2/f4/f8`, `b1`) — the full numpy fixed-width numeric
    /// ladder plus bool. Sub-16-bit codes (`i1/u1/b1/f2`) are stored **widened** to a
    /// pcodec-native width (free in output size) and read back at their original dtype —
    /// transparent to callers. For string data use a **table** block ([`add_table`]).
    ///
    /// `codec` selects the array codec, matching the Rust API (`tessera_io::array`):
    ///
    /// - `"pcodec"` (default) — the settled imaging-volume codec. On real CT/PET it compresses
    ///   ~3.8x vs zstd's ~3.3x, and it is what an acquisition-shaped volume wants.
    /// - `"zstd"` — Zarr v3 `bytes` + `zstd`. Wins on data with long byte-level repeats rather
    ///   than a numeric distribution: synthetic ramps, masks, packed bitfields.
    /// - `"auto"` — encode with both and keep the smaller, recording the winner in the manifest.
    ///   Readers never see `"auto"`; it costs a double encode at write time.
    ///
    /// All three are per-chunk codecs, so partial reads ([`Reader::read_array_subset`]) work
    /// identically whichever was used, and the manifest always records the concrete codec.
    #[pyo3(signature = (name, code, shape, data, codec="pcodec"))]
    fn add_array(
        &mut self,
        name: &str,
        code: &str,
        shape: Vec<u64>,
        data: &[u8],
        codec: &str,
    ) -> PyResult<()> {
        let arr = ArrayData::from_le_bytes(code, data).map_err(err)?;
        let n: u64 = shape.iter().product();
        if arr.len() as u64 != n {
            return Err(TesseraError::new_err(format!(
                "array '{name}': {} elements != shape product {n}",
                arr.len()
            )));
        }
        let mut spec = ArraySpec::new(shape, arr.dtype());
        // `ArraySpec` defaults to zstd; the array backend's own default is pcodec, and
        // `array_block` validates the value and rejects anything other than pcodec/zstd/auto.
        spec.codec = codec.into();
        let (block_ref, payload) =
            tessera_io::array::array_block(name, &spec, &arr).map_err(err)?;
        self.builder()?.add_block_ref(block_ref);
        self.payloads.push(payload);
        Ok(())
    }

    /// Add a **table** block from ordered `columns` of `(name, code, le_bytes)`. All columns must
    /// have equal length. `row_index` (optional) names the O(1)-take index column.
    ///
    /// Accepted `code`s: `i1/i2/i4/i8`, `u1/u2/u4/u8`, `f4/f8`, `b1` (one byte per bool), and
    /// `str` (UTF-8 length-prefixed: `[u32 LE len | bytes]*`). `str` is table-only; conversely
    /// float16 is array-only ([`add_array`]) — the columnar toolchain (polars/arrow/duckdb) has
    /// no native half-float, so float16 *columns* would silently upcast downstream.
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
                ..Default::default()
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

    /// Record a provenance edge (`relation`, e.g. `"ingested_from"` or `"derived_from"`, →
    /// `reference`).
    ///
    /// `reference` is the parent's lineage handle (a path, a URL, a product `id`). `content_hash`
    /// **pins the parent's seal** on the edge (SPEC §8) and must be the parent's `manifest_hash`:
    /// `tessera.open(parent).manifest()["manifest_hash"]`. It is what closes the integrity chain, so
    /// `tessera provenance --require-complete` reports an edge without it as a gap. It is not
    /// format-checked at write time (neither is the Rust `Source::with_content_hash`); a wrong value
    /// surfaces when the chain is walked. Omitted, the key is absent from the manifest, so a product
    /// written without it seals exactly as before.
    #[pyo3(signature = (relation, reference, content_hash=None))]
    fn add_source(
        &mut self,
        relation: &str,
        reference: &str,
        content_hash: Option<&str>,
    ) -> PyResult<()> {
        let mut source = tessera_core::provenance::Source::new(relation, reference);
        if let Some(hash) = content_hash {
            source = source.with_content_hash(hash);
        }
        self.builder()?.add_source(source);
        Ok(())
    }

    /// Declare the tool that **generated** this product (ADR-0058 §1): your DAQ, simulation, recon
    /// or pipeline, not the packager. Sealed into `manifest_hash`, so it is tamper-evident and part
    /// of the product's identity. Read it back as `manifest()["producer"]`, a dict with `tool` and
    /// `version` plus whichever of `git_commit` / `git_repo` / `dirty` were given (an omitted one is
    /// absent, not `None`).
    ///
    /// Without this call the product is stamped `{"tool": "tessera", "version": <format version>}`,
    /// which names the packager. Calling it again replaces the earlier producer.
    #[pyo3(signature = (tool, version, git_commit=None, git_repo=None, dirty=None))]
    fn with_producer(
        &mut self,
        tool: &str,
        version: &str,
        git_commit: Option<&str>,
        git_repo: Option<&str>,
        dirty: Option<bool>,
    ) -> PyResult<()> {
        let builder = self.builder()?;
        let mut producer = tessera_core::provenance::Producer::new(tool, version);
        producer.git_commit = git_commit.map(str::to_string);
        producer.git_repo = git_repo.map(str::to_string);
        producer.dirty = dirty;
        builder.with_producer(producer);
        Ok(())
    }

    /// Record **how** this product was made (ADR-0058 §2): the generator's recipe, sealed into
    /// `manifest_hash`. Read it back as `manifest()["generation"]`.
    ///
    /// - `config` is a dict of settings (`{"energy_window_keV": [425, 650], "agent": {...}}`). Its keys
    ///   are yours: tessera never interprets them. Keys must be `str` at every level, and values must
    ///   be JSON: `str`, `int` (64-bit), finite `float`, `bool`, `None`, `list`/`tuple` and `dict`.
    ///   Anything else raises `TypeError` naming the offending key, rather than being coerced
    ///   (`json.dumps` would turn `{1: ...}` into `{"1": ...}` without telling you). That includes a
    ///   `str` holding a lone surrogate (`"\ud800"`), which has no UTF-8 form. A container that
    ///   contains itself raises `ValueError`, and nesting deeper than `sys.getrecursionlimit()`
    ///   raises `RecursionError`, both as `json.dumps` does. The manifest reader parses about
    ///   125 levels of `config`; a deeper one seals but can't be opened (#589).
    /// - `config_ref` is a `blake3:` digest of a config block carried in the same `.tsra`. It is not
    ///   checked at seal; `tessera inspect --resolve-config` reports a reference that names no carried
    ///   block. The Python `Builder` cannot yet carry the Blob block such a reference points at
    ///   (#575), so from Python prefer `config`.
    ///
    /// Pass either or both. With neither, or with an empty `config` and no `config_ref`, this raises
    /// `TesseraError`: an empty recipe counts as *absent* (`Generation::is_empty`), so sealing one
    /// would let you believe a recipe was recorded when none was, and a schema with
    /// `requires_generation` would still reject the product. Calling it again replaces the earlier
    /// recipe.
    #[pyo3(signature = (config=None, config_ref=None))]
    fn with_generation(
        &mut self,
        config: Option<&Bound<'_, PyDict>>,
        config_ref: Option<&str>,
    ) -> PyResult<()> {
        let builder = self.builder()?;
        let mut generation = tessera_core::provenance::Generation::default();
        if let Some(config) = config {
            generation.config = config_to_json(config)?;
        }
        if let Some(digest) = config_ref {
            generation = generation.with_config_ref(digest);
        }
        if generation.is_empty() {
            return Err(TesseraError::new_err(
                "with_generation needs a recipe: a non-empty `config` and/or a `config_ref` \
                 (an empty generation record counts as absent, ADR-0058 §2)",
            ));
        }
        builder.with_generation(generation);
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

/// `"an"` before a vowel, for error messages that name a Python type (`an int`, `a set`).
fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// A `str` key or value at `path`, as UTF-8. A Python `str` can hold a lone surrogate
/// (`"\ud800"`), which has no UTF-8 and so no JSON form; that is the caller's bad input, raised as
/// the same `TypeError` as every other non-JSON value instead of a `UnicodeEncodeError`.
fn json_str(s: &Bound<'_, PyAny>, path: &str, what: &str) -> PyResult<String> {
    s.extract::<String>().map_err(|e| {
        if e.is_instance_of::<PyUnicodeEncodeError>(s.py()) {
            let repr = s.repr().map(|r| r.to_string()).unwrap_or_default();
            PyTypeError::new_err(format!(
                "generation {path}: {what} {repr} contains a lone surrogate, which has no UTF-8 \
                 (so no JSON) form"
            ))
        } else {
            e
        }
    })
}

/// A dict key inside the generation `config` at `path`, which must be a `str`: JSON object keys are
/// strings, and coercing `1` to `"1"` would seal a different key than the caller wrote.
fn json_key(key: &Bound<'_, PyAny>, path: &str) -> PyResult<String> {
    if !key.is_instance_of::<PyString>() {
        let ty = key.get_type().name()?.to_string();
        return Err(PyTypeError::new_err(format!(
            "generation {path}: key {} is {} {ty}, not a str",
            key.repr()?,
            article(&ty),
        )));
    }
    json_str(key, path, "key")
}

/// Convert a generation `config` dict to JSON. Strict: only types with an exact JSON equivalent
/// are accepted, so what is sealed is what the caller wrote.
///
/// The walk is **iterative**, with an explicit stack on the heap: the native stack stays the same
/// depth however deeply `config` nests. A recursive walk overflowed it and killed the interpreter
/// (SIGSEGV) on a self-referencing dict or a deep nesting. Guarding the recursion with
/// `Py_EnterRecursiveCall` doesn't fix that on CPython 3.12, which counts calls against a fixed C
/// limit rather than measuring the stack, and the native frames run out first. Nesting deeper than
/// `sys.getrecursionlimit()` raises `RecursionError`, and a container that contains itself raises
/// `ValueError`, both as `json.dumps` does.
fn config_to_json(config: &Bound<'_, PyDict>) -> PyResult<BTreeMap<String, serde_json::Value>> {
    let py = config.py();
    let limit: usize = py
        .import("sys")?
        .getattr("getrecursionlimit")?
        .call0()?
        .extract()?;
    let mut walk = JsonWalk {
        // The config dict itself is depth 1 and an ancestor of everything in it.
        ancestors: HashSet::from([config.as_ptr() as usize]),
        limit,
    };
    let mut out = BTreeMap::new();
    for (key, value) in config.iter() {
        let key = json_key(&key, "config")?;
        let path = format!("config[{key:?}]");
        out.insert(key, walk.convert(&value, path)?);
    }
    Ok(out)
}

/// The state of one [`config_to_json`] walk.
struct JsonWalk {
    /// The containers open on the current path (by object identity). Meeting one again is a cycle.
    /// Only ancestors count, so one object referenced twice (a DAG) converts twice, like
    /// `json.dumps`.
    ancestors: HashSet<usize>,
    /// The deepest nesting accepted, counting the config dict as 1: `sys.getrecursionlimit()`.
    limit: usize,
}

/// A converted value, or a container whose children still have to be converted.
enum Step<'py> {
    Leaf(serde_json::Value),
    Open(OpenContainer<'py>),
}

/// A dict or list being converted: the explicit stack frame of [`JsonWalk::convert`].
struct OpenContainer<'py> {
    id: usize,
    path: String,
    /// The children still to convert: `(Some(key), value)` for a dict, `(None, item)` for a list.
    children: std::vec::IntoIter<(Option<String>, Bound<'py, PyAny>)>,
    /// The next list index (for error paths).
    index: usize,
    /// The key of the dict child being converted.
    key: String,
    out: Collected,
}

enum Collected {
    Object(serde_json::Map<String, serde_json::Value>),
    Array(Vec<serde_json::Value>),
}

impl<'py> OpenContainer<'py> {
    /// The next child and its path, remembering its key for [`Self::attach`].
    fn next_child(&mut self) -> Option<(Bound<'py, PyAny>, String)> {
        let (key, child) = self.children.next()?;
        let path = match key {
            Some(key) => {
                let path = format!("{}[{key:?}]", self.path);
                self.key = key;
                path
            }
            None => {
                self.index += 1;
                format!("{}[{}]", self.path, self.index - 1)
            }
        };
        Some((child, path))
    }

    /// Store the converted value of the child [`Self::next_child`] last returned.
    fn attach(&mut self, value: serde_json::Value) {
        match &mut self.out {
            Collected::Object(map) => {
                map.insert(std::mem::take(&mut self.key), value);
            }
            Collected::Array(items) => items.push(value),
        }
    }

    fn finish(self) -> serde_json::Value {
        match self.out {
            Collected::Object(map) => serde_json::Value::Object(map),
            Collected::Array(items) => serde_json::Value::Array(items),
        }
    }
}

impl JsonWalk {
    /// Convert `root` (at `path`, depth 2: a value inside the config dict) without recursing.
    fn convert(&mut self, root: &Bound<'_, PyAny>, path: String) -> PyResult<serde_json::Value> {
        let mut top = match self.step(root, path, 2)? {
            Step::Leaf(value) => return Ok(value),
            Step::Open(open) => open,
        };
        // `top` is the container being filled; `stack` holds its open ancestors.
        let mut stack: Vec<OpenContainer<'_>> = Vec::new();
        loop {
            match top.next_child() {
                Some((child, path)) => match self.step(&child, path, stack.len() + 3)? {
                    Step::Leaf(value) => top.attach(value),
                    Step::Open(open) => stack.push(std::mem::replace(&mut top, open)),
                },
                None => {
                    self.ancestors.remove(&top.id);
                    let value = top.finish();
                    match stack.pop() {
                        Some(parent) => {
                            top = parent;
                            top.attach(value);
                        }
                        None => return Ok(value),
                    }
                }
            }
        }
    }

    /// Convert a scalar at `path`, or open a container at nesting `depth`.
    fn step<'py>(
        &mut self,
        value: &Bound<'py, PyAny>,
        path: String,
        depth: usize,
    ) -> PyResult<Step<'py>> {
        use serde_json::Value;
        let bad = |why: String| PyTypeError::new_err(format!("generation {path}: {why}"));
        // `bool` first: in Python it is a subclass of `int`.
        let leaf = if value.is_none() {
            Value::Null
        } else if value.is_instance_of::<PyBool>() {
            Value::Bool(value.extract()?)
        } else if value.is_instance_of::<PyInt>() {
            if let Ok(i) = value.extract::<i64>() {
                Value::from(i)
            } else if let Ok(u) = value.extract::<u64>() {
                Value::from(u)
            } else {
                return Err(bad(format!(
                    "int {} does not fit in 64 bits",
                    value.repr()?
                )));
            }
        } else if value.is_instance_of::<PyFloat>() {
            let f: f64 = value.extract()?;
            let n = serde_json::Number::from_f64(f);
            Value::Number(
                n.ok_or_else(|| bad(format!("float {f} is not finite, and JSON has no NaN/inf")))?,
            )
        } else if value.is_instance_of::<PyString>() {
            Value::String(json_str(value, &path, "str")?)
        } else if value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>() {
            let items = value.try_iter()?.map(|item| item.map(|item| (None, item)));
            let children = items.collect::<PyResult<Vec<_>>>()?;
            return self.open(value, path, depth, children, Collected::Array(Vec::new()));
        } else if let Ok(dict) = value.cast::<PyDict>() {
            let mut children = Vec::with_capacity(dict.len());
            for (key, item) in dict.iter() {
                children.push((Some(json_key(&key, &path)?), item));
            }
            let out = Collected::Object(serde_json::Map::new());
            return self.open(value, path, depth, children, out);
        } else {
            let ty = value.get_type().name()?.to_string();
            return Err(bad(format!(
                "{} {ty} is not a JSON value (use str, int, float, bool, None, list or dict)",
                article(&ty),
            )));
        };
        Ok(Step::Leaf(leaf))
    }

    /// Open the container `value` at `path` and nesting `depth`, refusing a cycle or a nesting
    /// past the limit.
    fn open<'py>(
        &mut self,
        value: &Bound<'py, PyAny>,
        path: String,
        depth: usize,
        children: Vec<(Option<String>, Bound<'py, PyAny>)>,
        out: Collected,
    ) -> PyResult<Step<'py>> {
        if depth > self.limit {
            return Err(PyRecursionError::new_err(format!(
                "generation {path}: maximum recursion depth exceeded while converting a generation \
                 config to JSON (nesting deeper than sys.getrecursionlimit() = {})",
                self.limit
            )));
        }
        let id = value.as_ptr() as usize;
        if !self.ancestors.insert(id) {
            return Err(PyValueError::new_err(format!(
                "generation {path}: circular reference (a container that contains itself)"
            )));
        }
        Ok(Step::Open(OpenContainer {
            id,
            path,
            children: children.into_iter(),
            index: 0,
            key: String::new(),
            out,
        }))
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

// The compiled extension is `tessera._native` (the `tessera` package's pure-Python `__init__.py` wraps
// it with the numpy/polars/pyarrow ergonomic surface). pyo3 ties the module init symbol to this fn name.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("TesseraError", m.py().get_type::<TesseraError>())?;
    m.add_class::<Reader>()?;
    m.add_class::<Builder>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(verify, m)?)?;
    Ok(())
}
