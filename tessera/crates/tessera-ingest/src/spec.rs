//! Declarative ingest spec (ADR-0035) — a TOML description of a multi-product acquisition that
//! the [`crate::engine`] runs into a sealed Tessera **collection** of `.tsra` products.
//!
//! ## Why declarative
//! Vendor formats arrive in many specific layouts (GE singles + 2p + 3p coincidences + recon, Siemens
//! binary, raw `.dat`, NIfTI, …). Hardcoding each LAYOUT in Rust grows linearly with vendor work —
//! but the underlying **backends** (hdf-compound, dicom, dicom-series, nifti, raw) are a closed
//! handful. A spec separates "what to ingest" (config, in-flight) from "how to decode" (Rust, closed
//! set). Adding a new dataset layout in a supported format = new TOML. Adding a new CONTAINER (a
//! novel binary file with its own bytes-on-disk) = a new Rust backend.
//!
//! ## Identity discipline (load-bearing)
//! - Per-product identity is `{product=<schema>, name=<spec.name>, timestamp=<spec.timestamp>}`
//!   normalised to UTC — the spec MUST provide name + timestamp explicitly (never `Local::now()` /
//!   filesystem mtimes), so re-running the same spec on the same data produces byte-identical
//!   products + collection (proven by the engine's determinism test).
//! - Member order follows the TOML `[[product]]` declaration order — preserved by
//!   `Vec<ProductSpec>` + the topological sort (stable, parents-first). The collection's
//!   `content_hash` is an MMR over members in this order, so order is part of identity.
//! - The **spec_hash** the engine flows into each member as `Source { role: "ingested_via_spec" }`
//!   is `blake3` over **canonical JSON of the parsed model** (RFC 8785 JCS via
//!   [`tessera_core::canonical`]), not over the raw TOML text. Whitespace / comments / key-order in
//!   the source file cannot change identity.
//!
//! ## Validation gates (parse time)
//! - `derived_from` references resolve to spec-local product names (in-spec only, v1).
//! - The product DAG is acyclic (Kahn's algorithm) — a cycle is a hard error, not a runtime crash.
//! - Product names are unique within the spec — a duplicate is a hard error.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tessera_core::collection::Role;
use tessera_core::{Error, Result};

/// The default `block_prefix` for an `hdf-compound` product — the GE 2p/3p layouts encode their
/// table under this name and the conformance corpus pins it, so it's both the SSoT and the floor
/// for backward compatibility.
pub const DEFAULT_BLOCK_PREFIX: &str = "events";

/// The default `row_index` column for an `hdf-compound` product — the GE 2p/3p timestamps live in
/// `ms` and the conformance corpus pins it. Other layouts (singles' `time_ps`, coin's `time_us`)
/// pass their own.
pub const DEFAULT_ROW_INDEX: &str = "ms";

/// The default per-slab read unit for `hdf-compound` streaming — the GE-HDF5 reader uses the same
/// constant. Spec-overridable so wide-row datasets can shrink the per-slab RAM footprint.
pub const DEFAULT_SLAB_ROWS: usize = crate::ge_hdf5::STREAM_SLAB_ROWS;

/// One declarative ingest spec — typically loaded from `.toml` via [`parse`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestSpec {
    pub collection: CollectionMeta,
    #[serde(default)]
    pub spec: SpecMeta,
    /// Members in declared order — `Vec` (not `HashMap`) so the TOML `[[product]]` order is
    /// preserved verbatim. Member order is part of the collection's identity.
    #[serde(default, rename = "product")]
    pub products: Vec<ProductSpec>,
}

/// Collection-level metadata — what the produced [`tessera_core::Collection`] is named, when it was
/// captured, and which study it belongs to. Required: `name` + `timestamp` (so the collection id is
/// deterministic across machines).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study: Option<String>,
}

/// Spec-level annotations — currently just a free-form description; reserved for future
/// schema-version / author fields that don't belong on the collection itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpecMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One member of a collection: identity (`name` + `timestamp` come from `collection` unless
/// overridden), role (raw / derived → drives WORM), the product schema (e.g. `recon`, `listmode`),
/// the in-spec parents this product is derived from, and the format-tagged decoder options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductSpec {
    pub name: String,
    pub role: Role,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Names of OTHER products in this same spec this one is derived from. v1: in-spec only —
    /// resolved by name at validate time; cycles + dangling refs are rejected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// Optional clean label for the `ingested_from` provenance edge — used in place of the input
    /// path (which on real clinical data is PHI-bearing: e.g. an 890-slice DICOM series embeds the
    /// patient name 890× into the sealed manifest if the full paths are kept). When `None`, the
    /// path is recorded verbatim as before (the legacy behavior). ADR-0040.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    /// Free-form per-product metadata, written into the manifest's `metadata` field map.
    /// Use this for the small handful of fd5 schema fields the engine doesn't compute itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    /// Generation record (ADR-0058 §2) — *how* this product was made: the producing tool's config
    /// as a generic bag. TOML: `[product.generation] config = { … }` or `config_ref = "blake3:…"`.
    /// Sealed into `manifest_hash`; required at validate for schemas that set `requires_generation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<tessera_core::Generation>,
    /// Producer identity (ADR-0058 §1) — the tool/build that generated this product. TOML:
    /// `[product.producer] tool = "ge-listmode-daq" version = "…"`. Overrides the default `tessera`
    /// stamp so an external DAQ/SIM records itself. Sealed. Always structured (a spec never writes a
    /// legacy bare string), so `Producer` not `ProducerRef`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<tessera_core::Producer>,
    /// Format-tagged decoder options. The `format` discriminator picks the backend; the rest of
    /// the fields are backend-specific. Flattened via `#[serde(flatten)]` so a TOML
    /// `format = "hdf-compound"` reads sibling fields (`input`, `dataset`, …) directly off the
    /// `[[product]]` table — no nested `[product.options]` boilerplate.
    #[serde(flatten)]
    pub options: FormatOptions,
}

/// Streaming policy for a bounded-memory-capable lane (`hdf-compound`, and the three table lanes
/// since #458). `Auto` = the engine decides per `stream_threshold`; explicit overrides force the
/// path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingMode {
    /// Engine decides: stream iff estimated payload (rows × row_bytes) > `stream_threshold`.
    #[default]
    Auto,
    /// Always batch (whole-file read → encode → seal). Tiny acquisitions; debug.
    Batch,
    /// Always stream (bounded-memory hyperslab → multi-block sink). Large acquisitions.
    Stream,
}

/// Backend-specific decoder options, tagged by `format = "…"` in TOML.
///
/// Adding a backend = a new variant + a dispatch arm in `engine::run`. Adding a new dataset LAYOUT
/// in an existing format = a new TOML (no Rust change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case")]
pub enum FormatOptions {
    Dicom {
        input: PathBuf,
        #[serde(default)]
        deidentify: bool,
        /// Crypto-shred de-identification (ADR-0047): `age` recipient public keys. When non-empty,
        /// the product is de-identified AND the stripped identity is encrypted to these keys and
        /// carried as an `aux/identity/identity.age` envelope (`deidentify` is then implied).
        #[serde(default)]
        recipients: Vec<String>,
    },
    DicomSeries {
        inputs: Vec<PathBuf>,
        #[serde(default)]
        deidentify: bool,
        /// Crypto-shred recipient `age` public keys (ADR-0047) — see [`FormatOptions::Dicom`].
        #[serde(default)]
        recipients: Vec<String>,
        /// How to encode a per-slice-rescaled series (#300): `bit-exact` (default) rejects differing
        /// `RescaleSlope`s; `global-int16` requantizes them to one int16 scale (GE quantitative PET).
        #[serde(default)]
        rescale_mode: crate::dicom::RescaleMode,
    },
    HdfCompound {
        input: PathBuf,
        dataset: String,
        #[serde(default = "default_row_index")]
        row_index: String,
        #[serde(default = "default_block_prefix")]
        block_prefix: String,
        #[serde(default)]
        streaming: StreamingMode,
        #[serde(default = "default_slab_rows")]
        slab_rows: usize,
        /// Opt-in GEDDF transform (#310): annotate columns (unit/description/short_name) + requantize
        /// float columns to int16 at the physical resolution (physical = raw × scale). Default off →
        /// byte-identical output. Forces the batch path (the transform needs the whole table).
        #[serde(default)]
        quantize: bool,
    },
    Nifti {
        input: PathBuf,
    },
    Raw {
        input: PathBuf,
        shape: Vec<u64>,
        dtype: String,
    },
    /// Opaque preservation: store the file's bytes **verbatim** (the "junk" tier — no decode). Use
    /// `format = "blob"`, or the cathartic `format = "junk"` alias when the vendor file has earned it.
    #[serde(alias = "junk")]
    Blob {
        input: PathBuf,
        /// IANA media type, if known (e.g. `application/pdf`). Defaults to opaque octet-stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    /// **Parquet → flat `table`** (ADR-0056 §11 P1). A logical re-encode: the source's compression,
    /// page layout and dictionary ordering are all dropped, so a snappy Parquet and a zstd Parquet of
    /// the same logical table seal to the same `content_hash`.
    Parquet {
        input: PathBuf,
        /// Source columns to drop before mapping — the escape hatch every §2 rejection names.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exclude: Vec<String>,
        /// Bounded-memory streaming (#458). `auto` streams when the input's estimated decoded size
        /// exceeds `stream_threshold`, mirroring how `hdf-compound` decides.
        ///
        /// A **runtime execution knob, not content.** Both paths seal identical bytes — the block
        /// partition is a function of the data alone (`tessera_io::block_count`), and the batch path
        /// goes through those same helpers — so this trades memory against a second read of the input
        /// and cannot move any sealed byte.
        ///
        /// Therefore **never serialised, at any value**: ADR-0035 hashes the parsed spec into every
        /// member's `ingested_via_spec` edge, so a field that rides `spec_hash` rides
        /// `manifest_hash`. Were this recorded, re-running an archived ingest with
        /// `streaming = "stream"` — on a smaller machine, which is exactly when an operator reaches
        /// for it — could not reproduce the archived `manifest_hash` for byte-identical data. A knob
        /// that cannot change the product must not be able to change the product's identity. It
        /// still parses, so an archived spec stays readable and the operator's choice is honoured.
        #[serde(default, skip_serializing)]
        streaming: StreamingMode,
        /// Rows per decoded batch on the streaming path — the read-side memory unit, independent of
        /// the block partition and of the seal. Never serialised, for the reason above.
        #[serde(default = "default_batch_rows", skip_serializing)]
        batch_rows: usize,
        /// Operator-declared per-column semantics (ADR-0056 §7), landing **inside the seal**.
        ///
        /// Inline rather than a path to a sidecar TOML, deliberately: ADR-0035 hashes the *parsed*
        /// spec into each member's `ingested_via_spec` edge, which makes a spec an archival artifact.
        /// A path would make that hash a promise about a file the spec does not contain.
        #[serde(
            default,
            skip_serializing_if = "crate::column_meta::ColumnMeta::is_empty"
        )]
        column_meta: crate::column_meta::ColumnMeta,
    },
    /// **Arrow IPC / Feather → flat `table`**. Same type map as `parquet`; only the container differs.
    Arrow {
        input: PathBuf,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exclude: Vec<String>,
        /// Bounded-memory streaming (#458). `auto` streams when the input's estimated decoded size
        /// exceeds `stream_threshold`, mirroring how `hdf-compound` decides.
        ///
        /// A **runtime execution knob, not content.** Both paths seal identical bytes — the block
        /// partition is a function of the data alone (`tessera_io::block_count`), and the batch path
        /// goes through those same helpers — so this trades memory against a second read of the input
        /// and cannot move any sealed byte.
        ///
        /// Therefore **never serialised, at any value**: ADR-0035 hashes the parsed spec into every
        /// member's `ingested_via_spec` edge, so a field that rides `spec_hash` rides
        /// `manifest_hash`. Were this recorded, re-running an archived ingest with
        /// `streaming = "stream"` — on a smaller machine, which is exactly when an operator reaches
        /// for it — could not reproduce the archived `manifest_hash` for byte-identical data. A knob
        /// that cannot change the product must not be able to change the product's identity. It
        /// still parses, so an archived spec stays readable and the operator's choice is honoured.
        #[serde(default, skip_serializing)]
        streaming: StreamingMode,
        /// Rows per decoded batch on the streaming path — the read-side memory unit, independent of
        /// the block partition and of the seal. Never serialised, for the reason above.
        #[serde(default = "default_batch_rows", skip_serializing)]
        batch_rows: usize,
        #[serde(
            default,
            skip_serializing_if = "crate::column_meta::ColumnMeta::is_empty"
        )]
        column_meta: crate::column_meta::ColumnMeta,
    },
    /// **CSV/TSV → flat `table`, under a DECLARED schema** (ADR-0056 §8).
    ///
    /// `columns` is required and total: Tessera infers no dtype, because an inferred schema depends on
    /// which rows were sampled and the seal must be reproducible. Declarations are positional and are
    /// cross-checked against the header row.
    Csv {
        input: PathBuf,
        /// `NAME:DTYPE` per column, in file order; `?` suffix allows NULLs (`age:i4?`).
        columns: Vec<String>,
        /// Field delimiter, as a one-character string (`","`, `"\t"`). Never sniffed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delimiter: Option<String>,
        /// Whether row 1 is a header to skip and check. Default `true`.
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        header: bool,
        /// Extra field texts read as NULL, beyond the always-NULL empty field (`"NA"`, `"NULL"`, …).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        null_tokens: Vec<String>,
        /// Declared columns to drop after reading (declarations stay positional).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exclude: Vec<String>,
        /// Bounded-memory streaming (#458). `auto` streams when the input's estimated decoded size
        /// exceeds `stream_threshold`, mirroring how `hdf-compound` decides.
        ///
        /// A **runtime execution knob, not content.** Both paths seal identical bytes — the block
        /// partition is a function of the data alone (`tessera_io::block_count`), and the batch path
        /// goes through those same helpers — so this trades memory against a second read of the input
        /// and cannot move any sealed byte.
        ///
        /// Therefore **never serialised, at any value**: ADR-0035 hashes the parsed spec into every
        /// member's `ingested_via_spec` edge, so a field that rides `spec_hash` rides
        /// `manifest_hash`. Were this recorded, re-running an archived ingest with
        /// `streaming = "stream"` — on a smaller machine, which is exactly when an operator reaches
        /// for it — could not reproduce the archived `manifest_hash` for byte-identical data. A knob
        /// that cannot change the product must not be able to change the product's identity. It
        /// still parses, so an archived spec stays readable and the operator's choice is honoured.
        #[serde(default, skip_serializing)]
        streaming: StreamingMode,
        /// Rows per decoded batch on the streaming path — the read-side memory unit, independent of
        /// the block partition and of the seal. Never serialised, for the reason above.
        #[serde(default = "default_batch_rows", skip_serializing)]
        batch_rows: usize,
        #[serde(
            default,
            skip_serializing_if = "crate::column_meta::ColumnMeta::is_empty"
        )]
        column_meta: crate::column_meta::ColumnMeta,
    },
    /// **NumPy `.npy` → a dense `array`** (ADR-0056 §11). Shape and dtype are explicit in the header,
    /// so there is nothing to infer; a **structured** dtype routes to the `table` primitive instead
    /// (§1: the shape of the data decides, and a record array is rows of typed fields).
    Npy {
        input: PathBuf,
    },
    /// **One member of a NumPy `.npz` archive → a dense `array`.**
    ///
    /// An `.npz` is a zip of `.npy` files, so §11 makes it a **collection** — one product per member,
    /// not one product with N blocks (§10's shape rule one level up: the members are independent
    /// arrays, not slices of one grid). Naming the member explicitly is what keeps the spec an honest
    /// archival record: a spec that said "everything in this archive" would have a meaning that
    /// depended on the archive.
    NpzMember {
        input: PathBuf,
        /// The member's name inside the archive, e.g. `energy.npy`.
        member: String,
    },
    /// Multi-file opaque preservation: seal a set of files as ONE `blob` product with a `Blob` block
    /// **per file** (no tar — the `.tsra` is already a STORED-zip container). The block-per-file cold
    /// tier for a multi-file vendor series (e.g. a DICOM series' slices). `format = "blob-series"`.
    BlobSeries {
        inputs: Vec<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
}

fn default_row_index() -> String {
    DEFAULT_ROW_INDEX.into()
}
fn default_block_prefix() -> String {
    DEFAULT_BLOCK_PREFIX.into()
}
/// Rows per decoded batch on the streaming table path.
///
/// 64 Ki = `ROWS_PER_GROUP`, so a batch lines up with the sink's row-group flush and no partial group is
/// carried between pushes. Not a determinism input — `the_batch_size_and_worker_count_never_move_a_seal`
/// pins that — so it is tunable without a corpus event.
fn default_batch_rows() -> usize {
    64 * 1024
}

fn default_slab_rows() -> usize {
    DEFAULT_SLAB_ROWS
}
fn default_true() -> bool {
    true
}
/// `skip_serializing_if` for a `bool` whose default is `true` — omit it when it holds the default, so a
/// spec's canonical JSON (and therefore its `spec_hash`) is unchanged by not mentioning the field.
fn is_true(b: &bool) -> bool {
    *b
}

/// Read + parse a `.toml` file into an [`IngestSpec`]. Does NOT validate — call [`validate`] next
/// (or use the engine, which runs both).
pub fn parse(path: &std::path::Path) -> Result<IngestSpec> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Invalid(format!("ingest-spec: read {}: {e}", path.display())))?;
    parse_str(&text)
}

/// Parse a TOML string into an [`IngestSpec`] — testable without touching the filesystem.
pub fn parse_str(toml_text: &str) -> Result<IngestSpec> {
    toml::from_str(toml_text).map_err(|e| Error::Invalid(format!("ingest-spec: parse: {e}")))
}

/// Validate the spec: unique product names; every `derived_from` reference resolves; the product
/// DAG is acyclic (Kahn's topological sort). Returns the topo order (parents-first) on success —
/// callers that don't need the order can ignore the `Vec`.
pub fn validate(spec: &IngestSpec) -> Result<Vec<usize>> {
    let n = spec.products.len();
    // 1. unique names — duplicates would silently shadow each other in any name→index map.
    let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, p) in spec.products.iter().enumerate() {
        if by_name.insert(p.name.as_str(), i).is_some() {
            return Err(Error::Invalid(format!(
                "ingest-spec: duplicate product name '{}'",
                p.name
            )));
        }
    }
    // 2. derived_from resolves — dangling refs are a config bug, never deferred to runtime.
    for p in &spec.products {
        for parent in &p.derived_from {
            if !by_name.contains_key(parent.as_str()) {
                return Err(Error::Invalid(format!(
                    "ingest-spec: product '{}' references unknown parent '{parent}' \
                     (derived_from is in-spec only — v1)",
                    p.name
                )));
            }
        }
    }
    // 3. acyclic DAG via Kahn's algorithm — a cycle would loop the engine forever.
    let mut indegree = vec![0usize; n];
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in spec.products.iter().enumerate() {
        for parent in &p.derived_from {
            let p_idx = by_name[parent.as_str()];
            children_of[p_idx].push(i);
            indegree[i] += 1;
        }
    }
    // Seed the queue with roots, IN DECLARED ORDER so the topo order is deterministic across
    // re-runs (BTreeSet would sort alphabetically — wrong; we want declared-order ties).
    let mut order = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    while let Some(i) = ready.first().copied() {
        ready.remove(0);
        order.push(i);
        // Stable: visit children IN DECLARED ORDER (already sorted by index since `children_of`
        // appends in declared order).
        for &c in &children_of[i] {
            indegree[c] -= 1;
            if indegree[c] == 0 {
                // Insert at the position that preserves declared order among the ready set.
                let pos = ready.iter().position(|&j| j > c).unwrap_or(ready.len());
                ready.insert(pos, c);
            }
        }
    }
    if order.len() != n {
        // Whatever wasn't visited is on a cycle; surface the offending names.
        let visited: BTreeSet<usize> = order.iter().copied().collect();
        let names: Vec<&str> = (0..n)
            .filter(|i| !visited.contains(i))
            .map(|i| spec.products[i].name.as_str())
            .collect();
        return Err(Error::Invalid(format!(
            "ingest-spec: cycle in derived_from involving products {names:?}"
        )));
    }
    // 4. ADR-0056 §7: the laundering rule.
    for p in &spec.products {
        check_no_schema_laundering(p)?;
        check_no_handwritten_decoder(p)?;
    }
    Ok(order)
}

/// **ADR-0056 §6a: nobody types the decoder record.** A spec-supplied `ingest_decoder` recipe key is a
/// hard error, for **every** backend.
///
/// §6a's whole argument against a decoder *profile id* was that it "substitutes a maintainer claim for a
/// verifiable fact inside an immutable record" — and a hand-written `ingest_decoder` is that same
/// substitution by a shorter road. The key's value is "a mechanically derived build-honest triple, every
/// component derived at build time… **Nobody types this string. It is derived, or it is not written.**"
///
/// Enforced for vendor backends too, and that is the case that actually matters. A generic lane would
/// overwrite a typed value anyway, so the damage there is limited to confusion; but a `dicom` or
/// `hdf-compound` product does not write a decoder record at all, so a spec-supplied one would seal
/// **unchanged** and be indistinguishable from a derived triple to every later reader. That is a sealed,
/// signed, unfalsifiable claim about how a file was interpreted — exactly what §6a exists to prevent.
fn check_no_handwritten_decoder(p: &ProductSpec) -> Result<()> {
    let Some(generation) = &p.generation else {
        return Ok(());
    };
    if !generation.config.contains_key(crate::decoder::RECIPE_KEY) {
        return Ok(());
    }
    Err(Error::Invalid(format!(
        "ingest-spec: product '{}' sets the '{}' recipe key by hand.\n  \
         That key is derived at build time — the decoder's name, its `=`-pinned version and a digest \
         over the resolved decode-path pins (ADR-0056 §6a) — and a typed value would be an \
         unfalsifiable claim about how the file was interpreted, sealed and signed alongside the data.\n  \
         remove it: the generic ingest lanes write it for you; a vendor backend deliberately writes \
         none rather than a guess.\n  \
         to record YOUR tool's own settings, use any other key in [product.generation.config] — that \
         is what the bag is for.",
        p.name,
        crate::decoder::RECIPE_KEY,
    )))
}

/// Resolve a table source's backend from its magic bytes, or explain what to pass (ADR-0056 §4).
///
/// Lives here rather than in the CLI so the one-product spec the CLI builds and a hand-written TOML
/// spec name the backend from the same string set — ADR-0056 §4's "`--from <name>` and the TOML
/// `from = \"<name>\"` are one string set and one grep locates every backend".
pub fn sniff_or_explain(input: &std::path::Path) -> Result<String> {
    match crate::canonical::sniff_table_format(input)? {
        Some(f) => Ok(f.to_string()),
        None => Err(crate::canonical::unknown_format_error(input)),
    }
}

/// Resolve an array source's backend from its magic bytes, or explain what to pass (ADR-0056 §4).
pub fn sniff_array_or_explain(input: &std::path::Path) -> Result<String> {
    match crate::canonical::sniff_array_format(input)? {
        Some(f) => Ok(f.to_string()),
        None => Err(crate::canonical::unknown_array_format_error(input)),
    }
}

/// The CSV-without-declarations refusal, routed through the one place that owns its wording.
///
/// Feature-gated indirection rather than a duplicated string: with `csv` compiled in, the message is
/// the canonical one from [`crate::csv_table::no_schema_error`]; without it, a build that cannot read
/// CSV at all should say *that* instead, since the declarations would not help.
pub fn csv_needs_declarations() -> Error {
    #[cfg(feature = "csv")]
    {
        crate::csv_table::no_schema_error()
    }
    #[cfg(not(feature = "csv"))]
    {
        Error::BackendNotCompiled("csv")
    }
}

/// The product schemas a **generic** backend is allowed to claim (ADR-0056 §7).
///
/// Named by primitive, because that is all a generic backend knows about its input: it read a flat
/// table, a dense grid, or opaque bytes.
pub const GENERIC_PRODUCT_SCHEMAS: &[&str] = &["table", "array", "blob"];

/// Is this backend a **generic** one — i.e. one that normalises a format with no domain semantics?
///
/// Exhaustive by construction: the `match` forces a new [`FormatOptions`] variant to declare which
/// side of the line it is on, so a future backend cannot slip through unclassified.
pub fn is_generic_backend(opts: &FormatOptions) -> bool {
    match opts {
        // Generic: the container tells us the shape and nothing about the domain.
        FormatOptions::Parquet { .. }
        | FormatOptions::Arrow { .. }
        | FormatOptions::Csv { .. }
        | FormatOptions::Npy { .. }
        | FormatOptions::NpzMember { .. }
        | FormatOptions::Blob { .. }
        | FormatOptions::BlobSeries { .. } => true,
        // Vendor: the decoder itself establishes the domain facts the schema then asserts (DICOM's
        // PS3.15 tag classification, GE's GEDDF dictionary, NIfTI's sform/qform frame).
        FormatOptions::Dicom { .. }
        | FormatOptions::DicomSeries { .. }
        | FormatOptions::HdfCompound { .. }
        | FormatOptions::Nifti { .. } => false,
        // `raw` is genuinely generic in nature — ADR-0056 §4 says it "was always a headerless
        // `array`" — but it is deliberately NOT swept in here. It still produces a `recon` product
        // (`raw::to_recon_product`), and the shipped `docs/examples/migrate-petct-study.toml` declares
        // `format = "raw"` with `schema = "recon"`. Enforcing the rule on it would therefore break a
        // documented workflow and move existing goldens, which belongs to §4's vendor-verb collapse (#456) —
        // a separate, mechanical change — not to this one. Until then, `raw` keeps the vendor
        // exemption it has always had in practice.
        FormatOptions::Raw { .. } => false,
    }
}

/// **ADR-0056 §7's laundering rule**: a generic backend may only produce a product in
/// [`GENERIC_PRODUCT_SCHEMAS`].
///
/// # Why this is a hard error and not a warning
///
/// The seal binds a schema's *promises*, not the pipeline's fulfilment of them. Without this check,
/// `tessera ingest table events.csv --schema listmode` produces a `.tsra` that is
/// **byte-indistinguishable** from a real vendor listmode ingest — carrying the `listmode` schema's
/// PS3.15 `Identifying` sensitivity tiers that *no classification pass ever validated*. Every
/// downstream consumer of `manifest.schema.fields[].sensitivity` would then trust them. That is the
/// self-describing-artifact thesis turned into a weapon.
///
/// It is enforced **here**, at parse time, rather than read back off a manifest field, for two
/// reasons: the engine knows which backend it dispatched to (a manifest only knows what it claims),
/// and failing before a single byte is read means a bad spec has no side effects.
///
/// It is also enforced **now** rather than in a later phase, contradicting ADR-0056's own landing
/// plan, because §7 says why: retrofitting it after the first such artifact ships would leave those
/// seals permanently ambiguous — nobody could tell a laundered `listmode` from a real one.
///
/// The designed escape hatch is `--classification-ack`, an ADR-0037 signature over the
/// (schema, column-tier) tuple. Its ergonomics are explicitly undesigned in the ADR, so it is not
/// built here; the error names the constraint instead of offering a flag that does not exist.
fn check_no_schema_laundering(p: &ProductSpec) -> Result<()> {
    if !is_generic_backend(&p.options) {
        return Ok(());
    }
    // The allowlist alone is not enough: `schema = "array"` on a Parquet product is *in* the allowlist
    // but the parquet lane builds a `table`, so the engine would seal a product whose declared schema
    // and actual block kind disagree — caught later by `schema.validate()` as a confusing block-kind
    // error, or worse, not caught if the schemas ever converge. A generic backend must claim **its own**
    // primitive.
    let expected = default_schema_for(&p.options);
    if p.schema == expected {
        return Ok(());
    }
    // The one case where the primitive depends on the FILE and not the format: a NumPy **structured**
    // dtype is rows of typed fields, so a `.npy` may legitimately produce either primitive (§1/§11). An
    // operator cannot know which without opening the file, so both are accepted here and the engine
    // seals whichever the header held.
    if matches!(
        p.options,
        FormatOptions::Npy { .. } | FormatOptions::NpzMember { .. }
    ) && p.schema == "table"
    {
        return Ok(());
    }
    if GENERIC_PRODUCT_SCHEMAS.contains(&p.schema.as_str()) {
        return Err(Error::Invalid(format!(
            "ingest-spec: product '{}' is read by the '{}' backend, which produces a '{expected}', but \
             declares schema '{}'.\n  \
             Both are primitive schemas, so this is not a laundering attempt — it is a mismatch that \
             would seal a product whose declared contract and actual block kind disagree.\n  \
             use:  schema = \"{expected}\"",
            p.name,
            crate::backends::backend_name(&p.options),
            p.schema,
        )));
    }
    Err(Error::Invalid(format!(
        "ingest-spec: product '{}' is read by the generic '{}' backend but claims schema '{}'.\n  A generic backend has no domain knowledge, so it cannot honour a domain schema's promises — \
         and a `.tsra` carrying that schema's sensitivity tiers would be indistinguishable from one a \
         real vendor ingest classified (ADR-0056 §7).\n  use a primitive schema:  schema = \"{}\"\n  or ingest through the vendor backend that knows the domain (format = \"dicom\" / \"ge-hdf5\" \
         / \"nifti\"), which classifies at the door",
        p.name,
        crate::backends::backend_name(&p.options),
        p.schema,
        default_schema_for(&p.options),
    )))
}

/// The primitive schema a generic backend's output belongs to — what the CLI stamps and what the
/// laundering error suggests.
pub fn default_schema_for(opts: &FormatOptions) -> &'static str {
    match opts {
        FormatOptions::Parquet { .. } | FormatOptions::Arrow { .. } | FormatOptions::Csv { .. } => {
            "table"
        }
        // `npy` is the one generic backend whose primitive depends on the FILE: a structured/record
        // dtype is a table (§1/§11). The spec declares `array`, and the engine relaxes the check for
        // exactly that case rather than making an operator predict their own dtype.
        FormatOptions::Npy { .. } | FormatOptions::NpzMember { .. } => "array",
        // See `is_generic_backend`: `raw` is not subject to the rule yet, so this value is
        // informational only (it is what §4's collapse will make it).
        FormatOptions::Raw { .. } => "array",
        FormatOptions::Blob { .. } | FormatOptions::BlobSeries { .. } => "blob",
        // A vendor backend's schema is the operator's declaration, not ours to default; the value is
        // only ever read for generic backends (see `check_no_schema_laundering`).
        FormatOptions::Dicom { .. }
        | FormatOptions::DicomSeries { .. }
        | FormatOptions::HdfCompound { .. }
        | FormatOptions::Nifti { .. } => "recon",
    }
}

/// Canonical-JSON bytes of the parsed spec — what [`spec_hash`] hashes over. Whitespace / comments
/// in the TOML source cannot change these bytes (the parsed model is what's serialised).
pub fn canonical_bytes(spec: &IngestSpec) -> Result<Vec<u8>> {
    tessera_core::canonical::to_bytes(spec)
}

/// `blake3` over the canonical-JSON bytes of the parsed spec — the engine threads this into each
/// produced member as `Source { role: "ingested_via_spec", reference: <spec_path>, content_hash:
/// Some(<spec_hash>) }`. Re-running the same TOML on the same data must produce the same hash.
pub fn spec_hash(spec: &IngestSpec) -> Result<String> {
    Ok(tessera_core::hash::digest(&canonical_bytes(spec)?))
}

#[cfg(test)]
mod streaming_knob_tests {
    use super::*;

    /// The streaming knobs must not reach `spec_hash` **at any value**.
    ///
    /// ADR-0035 hashes the parsed spec into every member's `ingested_via_spec` edge, so a field in
    /// `spec_hash` is a field in `manifest_hash`. `streaming` and `batch_rows` are runtime execution
    /// knobs: the block partition is a function of the data alone, so neither can move a sealed
    /// byte. Recording them would mean an operator who re-runs an archived ingest with
    /// `--streaming stream` — on a smaller machine, which is precisely why one reaches for it —
    /// could not reproduce the archived `manifest_hash` for byte-identical data.
    ///
    /// An earlier version of this test only required the *defaults* to be absent. That was too weak
    /// in exactly the direction that mattered, and it let a knob stay identity-bearing whenever
    /// somebody used it.
    #[test]
    fn the_streaming_knobs_never_reach_the_spec_hash() {
        let spec = |extra: &str| {
            parse_str(&format!(
                r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "p"
role = "raw"
schema = "table"
format = "csv"
input = "x.csv"
columns = ["a:i4"]
{extra}
"#
            ))
            .expect("parse")
        };
        let base = spec_hash(&spec("")).unwrap();
        for extra in [
            "streaming = \"auto\"",
            "streaming = \"batch\"",
            "streaming = \"stream\"",
            "batch_rows = 4096",
            "batch_rows = 65536",
            "streaming = \"stream\"\nbatch_rows = 1024",
        ] {
            assert_eq!(
                spec_hash(&spec(extra)).unwrap(),
                base,
                "`{extra}` changed the spec hash; it is an execution knob and must be invisible to \
                 the archival record"
            );
        }
        // The guard on the guard: a field that genuinely IS content still moves the hash, so the
        // assertion above cannot be satisfied by a spec hash that ignores everything.
        assert_ne!(
            spec_hash(&spec("exclude = [\"a\"]")).unwrap(),
            base,
            "a real decoder option must still ride the spec hash"
        );
    }

    /// The knobs still PARSE — "not recorded" must not become "not honoured".
    #[test]
    fn the_streaming_knobs_are_still_read_from_the_toml() {
        let parsed = parse_str(
            r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "p"
role = "raw"
schema = "table"
format = "csv"
input = "x.csv"
columns = ["a:i4"]
streaming = "stream"
batch_rows = 4096
"#,
        )
        .expect("parse");
        match &parsed.products[0].options {
            FormatOptions::Csv {
                streaming,
                batch_rows,
                ..
            } => {
                assert_eq!(*streaming, StreamingMode::Stream);
                assert_eq!(*batch_rows, 4096);
            }
            other => panic!("expected the csv lane, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_toml() -> &'static str {
        r#"
[collection]
name = "DP06-study"
description = "DUPLET DP06 PET/CT study"
timestamp = "2024-01-01T00:00:00Z"
study = "DP06-2024-01"

[spec]
description = "GE listmode + CT recon as one collection"

[[product]]
name = "DP06-singles"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "fixtures/singles.h5"
dataset = "singles"
row_index = "time_ps"
block_prefix = "singles"

[[product]]
name = "DP06-coin-2p"
role = "raw"
schema = "listmode"
derived_from = ["DP06-singles"]
format = "hdf-compound"
input = "fixtures/coin_2p.h5"
dataset = "events_2p"
"#
    }

    #[test]
    fn parse_round_trips_a_golden_toml() {
        let s = parse_str(sample_toml()).unwrap();
        assert_eq!(s.collection.name, "DP06-study");
        assert_eq!(s.collection.timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(s.collection.study.as_deref(), Some("DP06-2024-01"));
        assert_eq!(s.products.len(), 2);
        // declared order preserved (singles, then coin_2p) — load-bearing for collection identity.
        assert_eq!(s.products[0].name, "DP06-singles");
        assert_eq!(s.products[1].name, "DP06-coin-2p");
        assert_eq!(s.products[1].derived_from, vec!["DP06-singles".to_string()]);
        match &s.products[0].options {
            FormatOptions::HdfCompound {
                row_index,
                block_prefix,
                slab_rows,
                streaming,
                ..
            } => {
                assert_eq!(row_index, "time_ps"); // overrides the default
                assert_eq!(block_prefix, "singles");
                assert_eq!(*slab_rows, DEFAULT_SLAB_ROWS);
                assert_eq!(*streaming, StreamingMode::Auto);
            }
            other => panic!("expected HdfCompound, got {other:?}"),
        }
        match &s.products[1].options {
            FormatOptions::HdfCompound {
                row_index,
                block_prefix,
                ..
            } => {
                assert_eq!(row_index, DEFAULT_ROW_INDEX); // default kept
                assert_eq!(block_prefix, DEFAULT_BLOCK_PREFIX);
            }
            other => panic!("expected HdfCompound, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        let toml = r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "dup"
role = "raw"
schema = "listmode"
format = "raw"
input = "x"
shape = [1]
dtype = "i2"

[[product]]
name = "dup"
role = "raw"
schema = "listmode"
format = "raw"
input = "y"
shape = [1]
dtype = "i2"
"#;
        let s = parse_str(toml).unwrap();
        let err = validate(&s).unwrap_err();
        assert!(
            format!("{err}").contains("duplicate product name"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_rejects_dangling_derived_from() {
        let toml = r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "child"
role = "derived"
schema = "recon"
derived_from = ["ghost"]
format = "raw"
input = "x"
shape = [1]
dtype = "i2"
"#;
        let s = parse_str(toml).unwrap();
        let err = validate(&s).unwrap_err();
        assert!(
            format!("{err}").contains("unknown parent 'ghost'"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_rejects_a_cycle() {
        let toml = r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "a"
role = "derived"
schema = "recon"
derived_from = ["b"]
format = "raw"
input = "x"
shape = [1]
dtype = "i2"

[[product]]
name = "b"
role = "derived"
schema = "recon"
derived_from = ["a"]
format = "raw"
input = "y"
shape = [1]
dtype = "i2"
"#;
        let s = parse_str(toml).unwrap();
        let err = validate(&s).unwrap_err();
        assert!(
            format!("{err}").contains("cycle in derived_from"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_returns_topological_order_parents_first() {
        // 3 products: A (root), B derived from A, C derived from A AND B → topo: A, B, C
        let toml = r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "A"
role = "raw"
schema = "listmode"
format = "raw"
input = "a"
shape = [1]
dtype = "i2"

[[product]]
name = "B"
role = "derived"
schema = "listmode"
derived_from = ["A"]
format = "raw"
input = "b"
shape = [1]
dtype = "i2"

[[product]]
name = "C"
role = "derived"
schema = "recon"
derived_from = ["A", "B"]
format = "raw"
input = "c"
shape = [1]
dtype = "i2"
"#;
        let s = parse_str(toml).unwrap();
        let order = validate(&s).unwrap();
        let names: Vec<&str> = order.iter().map(|&i| s.products[i].name.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
    }

    #[test]
    fn spec_hash_is_independent_of_source_whitespace_and_comments() {
        let bare = r#"
[collection]
name = "c"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "p"
role = "raw"
schema = "recon"
format = "raw"
input = "x"
shape = [1]
dtype = "i2"
"#;
        let messy = r#"
# the collection spec
[collection]
name    =   "c"

timestamp = "2024-01-01T00:00:00Z"


# the product entry
[[product]]
# raw file
name = "p"
role = "raw"
schema = "recon"
format = "raw"
input = "x"
shape = [1]
dtype = "i2"
"#;
        let h_bare = spec_hash(&parse_str(bare).unwrap()).unwrap();
        let h_messy = spec_hash(&parse_str(messy).unwrap()).unwrap();
        assert_eq!(
            h_bare, h_messy,
            "spec_hash MUST be over the parsed model, not the raw TOML"
        );
        // and it really is content-derived: changing data changes the hash.
        let changed = bare.replace("\"x\"", "\"y\"");
        let h_changed = spec_hash(&parse_str(&changed).unwrap()).unwrap();
        assert_ne!(h_bare, h_changed);
        assert!(h_bare.starts_with("blake3:"));
    }

    /// The committed example TOML (`tessera/docs/examples/ingest-ge-listmode.toml`) must parse +
    /// validate in this build. Catches docs/source drift cheaply. Embedded via `include_str!` so it
    /// works in the hermetic gate (the crane source snapshot includes `docs/examples/` — flake.nix)
    /// without a runtime path that escapes the sandbox.
    #[test]
    fn committed_example_toml_parses_and_validates() {
        let text = include_str!("../../../docs/examples/ingest-ge-listmode.toml");
        let parsed = parse_str(text).expect("example TOML must parse");
        let order = validate(&parsed).expect("example must validate (topo + uniqueness + DAG)");
        // Sanity: the example has the four GE products documented in the file.
        assert_eq!(parsed.products.len(), 4);
        assert_eq!(order.len(), 4);
        // The recon derives from the 2p coin product → must be later in topo than its parent.
        let pos_2p = order
            .iter()
            .position(|&i| parsed.products[i].name == "DP06-coin-2p")
            .expect("DP06-coin-2p in topo order");
        let pos_recon = order
            .iter()
            .position(|&i| parsed.products[i].name == "DP06-recon")
            .expect("DP06-recon in topo order");
        assert!(
            pos_2p < pos_recon,
            "parent (coin-2p) must precede derived (recon)"
        );
        // spec_hash is deterministic — re-parsing yields the same hash.
        let h1 = spec_hash(&parsed).unwrap();
        let h2 = spec_hash(&parse_str(text).unwrap()).unwrap();
        assert_eq!(h1, h2);
    }

    /// The PET/CT migration template (`docs/examples/migrate-petct-study.toml`, spike #235) must
    /// parse + validate in this build. Same drift-catching guarantee as the GE example test above,
    /// applied to the canonical migration shape: **blob → recon-ct → recon-pt** with `derived_from`
    /// edges + `[product.metadata] study=` on every member. The active members in the file are the
    /// three uncommented ones; the optional `roi-lesion` member is gated behind a real roi backend.
    #[test]
    fn committed_petct_migration_toml_parses_and_validates() {
        let text = include_str!("../../../docs/examples/migrate-petct-study.toml");
        let parsed = parse_str(text).expect("PET/CT migration template TOML must parse");
        let order = validate(&parsed).expect("template must validate (topo + uniqueness + DAG)");
        // Exactly the three active members (vendor-raw blob, recon-ct, recon-pt). If a future
        // edit adds/removes one, update this number AND the assertions below — the count anchors
        // the shape.
        assert_eq!(parsed.products.len(), 3, "active migration members");
        assert_eq!(order.len(), 3);
        // The integrity-chain shape: blob is a root (declared first, topo first); recon-ct
        // derives from blob; recon-pt derives from BOTH blob (raw lineage) AND recon-ct (AC-CT).
        let pos = |name: &str| -> usize {
            order
                .iter()
                .position(|&i| parsed.products[i].name == name)
                .unwrap_or_else(|| panic!("{name} in topo order"))
        };
        assert!(pos("vendor-raw") < pos("recon-ct"));
        assert!(pos("vendor-raw") < pos("recon-pt"));
        assert!(pos("recon-ct") < pos("recon-pt"));
        // Cohort-pruneability anchor: EVERY active member declares `[product.metadata] study=`
        // (the contract the spike is built around).
        for p in &parsed.products {
            assert_eq!(
                p.metadata.get("study"),
                Some(&serde_json::json!("STUDY-EXAMPLE-2024-01")),
                "member '{}' must declare study= in [product.metadata] (cohort-pruneable)",
                p.name
            );
        }
        // The blob member's role is `raw` (the source-of-record); recon-pt is `derived` (built
        // from raw + the CT). Roles drive WORM under the hood (ADR-0028).
        let by_name: std::collections::BTreeMap<&str, &ProductSpec> = parsed
            .products
            .iter()
            .map(|p| (p.name.as_str(), p))
            .collect();
        assert_eq!(by_name["vendor-raw"].role, Role::Raw);
        assert_eq!(by_name["recon-pt"].role, Role::Derived);
        // spec_hash deterministic.
        assert_eq!(
            spec_hash(&parsed).unwrap(),
            spec_hash(&parse_str(text).unwrap()).unwrap()
        );
    }
}
