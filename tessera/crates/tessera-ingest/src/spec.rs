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
/// constant. Spec-overrideable so wide-row datasets can shrink the per-slab RAM footprint.
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
    /// Format-tagged decoder options. The `format` discriminator picks the backend; the rest of
    /// the fields are backend-specific. Flattened via `#[serde(flatten)]` so a TOML
    /// `format = "hdf-compound"` reads sibling fields (`input`, `dataset`, …) directly off the
    /// `[[product]]` table — no nested `[product.options]` boilerplate.
    #[serde(flatten)]
    pub options: FormatOptions,
}

/// Streaming policy for an `hdf-compound` product. `Auto` = the engine decides per
/// `stream_threshold`; explicit overrides force the path.
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
    },
    DicomSeries {
        inputs: Vec<PathBuf>,
        #[serde(default)]
        deidentify: bool,
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
}

fn default_row_index() -> String {
    DEFAULT_ROW_INDEX.into()
}
fn default_block_prefix() -> String {
    DEFAULT_BLOCK_PREFIX.into()
}
fn default_slab_rows() -> usize {
    DEFAULT_SLAB_ROWS
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
    Ok(order)
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
}
