//! Provenance as a DAG — each [`Source`] is a typed edge to an upstream artifact.
//!
//! e.g. a `recon` product has a `Source { role: "ingested_from", reference: "<DICOM path>",
//! content_hash: Some(...) }`; a lifetime `spectrum` has a `Source` to the `listmode` product
//! it was histogrammed from. This is fd5's `sources/` model, carried into the manifest.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Typed role of this edge, e.g. "ingested_from", "emission_data", "calibration".
    pub role: String,
    /// Identifier or path of the upstream artifact.
    pub reference: String,
    /// Content hash of the upstream artifact, when known (closes the integrity chain). Per SPEC §8
    /// this is the parent product's `manifest_hash` — the seal the edge commits to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl Source {
    pub fn new(role: impl Into<String>, reference: impl Into<String>) -> Self {
        Source {
            role: role.into(),
            reference: reference.into(),
            content_hash: None,
        }
    }

    /// Builder: pin the upstream's seal hash on this edge (closes the integrity chain).
    pub fn with_content_hash(mut self, hash: impl Into<String>) -> Self {
        self.content_hash = Some(hash.into());
        self
    }
}

/// Structured producer identity (ADR-0058 §1) — *who/what generated a product*. The universal,
/// domain-agnostic keys the format fixes; a generator (tessera itself, or an external DAQ/SIM/recon)
/// fills its own. Sealed inside the manifest, so it is tamper-evident and part of the product's id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Producer {
    /// The generating tool, e.g. `tessera`, `ge-listmode-daq`, a sim name.
    pub tool: String,
    pub version: String,
    /// Exact source commit of the tool, when known. tessera stamps its own via the
    /// `TESSERA_GIT_COMMIT` build env (absent in a sandboxed build ⇒ `None`, keeping the build
    /// deterministic); an external producer fills its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repo: Option<String>,
    /// Working-tree dirty state at build, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

impl Producer {
    /// An external producer's minimal identity (tool + version).
    pub fn new(tool: impl Into<String>, version: impl Into<String>) -> Self {
        Producer {
            tool: tool.into(),
            version: version.into(),
            git_commit: None,
            git_repo: None,
            dirty: None,
        }
    }

    /// Tessera's own identity, stamped at seal. `git_commit` is captured from the optional
    /// `TESSERA_GIT_COMMIT` build env — `None` when unset (e.g. the Nix sandbox), so the default
    /// stamp stays writer-deterministic within a build.
    pub fn tessera() -> Self {
        Producer {
            tool: "tessera".into(),
            version: crate::manifest::TESSERA_VERSION.into(),
            git_commit: option_env!("TESSERA_GIT_COMMIT").map(str::to_string),
            git_repo: option_env!("TESSERA_GIT_REPO").map(str::to_string),
            dirty: None,
        }
    }
}

/// The manifest `producer` slot: a structured [`Producer`] (ADR-0058) **or** a legacy bare string
/// (`"tessera/0.0.0"`, pre-ADR-0058). Untagged so a legacy string round-trips **byte-identically**
/// — existing seals hold, no corpus break — while newly-sealed products carry the struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProducerRef {
    Structured(Producer),
    Legacy(String),
}

impl ProducerRef {
    /// Tessera's own structured identity (the default seal stamp).
    pub fn tessera() -> Self {
        ProducerRef::Structured(Producer::tessera())
    }

    /// The generating tool name, whichever form. A legacy `"tool/version"` splits on the first `/`.
    pub fn tool(&self) -> &str {
        match self {
            ProducerRef::Structured(p) => &p.tool,
            ProducerRef::Legacy(s) => s.split('/').next().unwrap_or(s),
        }
    }

    /// The tool version, whichever form.
    pub fn version(&self) -> &str {
        match self {
            ProducerRef::Structured(p) => &p.version,
            ProducerRef::Legacy(s) => s.split_once('/').map(|(_, v)| v).unwrap_or(""),
        }
    }

    /// One-line `tool/version` for display (round-trips a legacy string verbatim).
    pub fn display(&self) -> String {
        match self {
            ProducerRef::Structured(p) => match &p.git_commit {
                Some(c) => format!("{}/{} ({c})", p.tool, p.version),
                None => format!("{}/{}", p.tool, p.version),
            },
            ProducerRef::Legacy(s) => s.clone(),
        }
    }
}

/// Generation record (ADR-0058 §2) — *how a product was made*: a generic, **non-opinionated bag**.
/// The format enforces only that it is present + non-empty for products whose schema requires a
/// recipe; the `config` keys are the generator's business and are never inspected by the engine.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Generation {
    /// Free-form settings the generator used (energy window, coincidence window, TOF cal, quant
    /// scales, sim seed, cmd, …). Keys are opaque to the format.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, serde_json::Value>,
    /// Alternative to inline `config`: a `blake3:` digest of a config / `.ini` / `.cfg` **block
    /// carried in this `.tsra`** (ADR-0038 Blob) — bit-faithful and dedup'd for large vendor config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_ref: Option<String>,
}

impl Generation {
    /// A recipe is "present" iff it carries inline settings or points at a carried config block.
    pub fn is_empty(&self) -> bool {
        self.config.is_empty() && self.config_ref.is_none()
    }

    /// Builder: inline settings.
    pub fn with(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.config.insert(key.into(), value);
        self
    }

    /// Builder: point at a carried config block by digest.
    pub fn with_config_ref(mut self, digest: impl Into<String>) -> Self {
        self.config_ref = Some(digest.into());
        self
    }
}

/// Copy **inheritable identity** fields from `parent` into `child`'s metadata (ADR-0058 §5),
/// schema-driven: a field flows iff the `schema` marks it [`inherit`](crate::schema::FieldSpec::inherit)
/// **and** the child does not already set it (an explicit child value always wins). The engine holds
/// no field list — *which* fields are identity is the schema's declaration. The first-class `study`
/// grouping key (a manifest field, not schema metadata) is inherited when unset, as it is the
/// format's own grouping primitive rather than domain opinion.
pub fn inherit_identity(
    child: &mut Manifest,
    parent: &Manifest,
    schema: &crate::schema::ProductSchema,
) {
    for f in schema.inheritable_fields() {
        if !child.metadata.contains_key(&f.id) {
            if let Some(v) = parent.metadata.get(&f.id) {
                child.metadata.insert(f.id.clone(), v.clone());
            }
        }
    }
    if child.study.is_none() {
        child.study = parent.study.clone();
    }
}

/// Resolves a provenance `reference` (a parent product's `id`) to its manifest, so a chain can be
/// walked and verified. A `BTreeMap<id, Manifest>` is the simplest implementation; a real store
/// would fetch from object storage.
pub trait Resolver {
    fn resolve(&self, reference: &str) -> Option<Manifest>;
}

impl Resolver for BTreeMap<String, Manifest> {
    fn resolve(&self, reference: &str) -> Option<Manifest> {
        self.get(reference).cloned()
    }
}

/// Verify the provenance chain rooted at `manifest`: every [`Source`] edge that carries a
/// `content_hash` **and** resolves to a parent product must have that hash equal the parent's
/// `manifest_hash` (SPEC §8 — the edge commits to the parent's seal), recursively to the roots.
/// Edges that don't resolve (external leaves — e.g. a raw DICOM SOP UID or file path) are skipped:
/// they are acquisition provenance, not Tessera products. A genuine cycle is a hard error.
pub fn verify_chain<R: Resolver>(manifest: &Manifest, resolver: &R) -> Result<()> {
    verify_walk(manifest, resolver, &mut BTreeSet::new())
}

fn verify_walk<R: Resolver>(
    m: &Manifest,
    resolver: &R,
    on_stack: &mut BTreeSet<String>,
) -> Result<()> {
    if !on_stack.insert(m.id.clone()) {
        return Err(Error::Invalid(format!(
            "provenance cycle detected at product '{}'",
            m.id
        )));
    }
    for s in &m.sources {
        let (Some(expected), Some(parent)) = (&s.content_hash, resolver.resolve(&s.reference))
        else {
            continue; // no pinned hash, or an external (non-Tessera) leaf reference
        };
        let actual = parent.manifest_hash.clone().unwrap_or_default();
        if &actual != expected {
            return Err(Error::Integrity {
                what: "provenance_edge",
                expected: expected.clone(),
                actual,
            });
        }
        verify_walk(&parent, resolver, on_stack)?;
    }
    on_stack.remove(&m.id); // pop: diamonds (a node reached by two paths) are fine; only cycles fail
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProductBuilder;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn sealed(name: &str) -> Manifest {
        ProductBuilder::new("recon", name, "d", TS).seal().unwrap()
    }

    #[test]
    fn verify_chain_accepts_correct_edge_and_skips_external_leaf() {
        let parent = sealed("parent");
        let mut store = BTreeMap::new();
        store.insert(parent.id.clone(), parent.clone());

        let mut cb = ProductBuilder::new("recon", "child", "d", TS);
        cb.add_source(
            Source::new("derived_from", &parent.id)
                .with_content_hash(parent.manifest_hash.clone().unwrap()),
        );
        cb.add_source(Source::new("ingested_from", "/scanner/raw.dcm")); // external leaf — skipped
        let child = cb.seal().unwrap();

        verify_chain(&child, &store).unwrap();
    }

    #[test]
    fn verify_chain_rejects_a_tampered_edge() {
        let parent = sealed("parent");
        let mut store = BTreeMap::new();
        store.insert(parent.id.clone(), parent.clone());

        let mut cb = ProductBuilder::new("recon", "child", "d", TS);
        cb.add_source(Source::new("derived_from", &parent.id).with_content_hash("blake3:deadbeef"));
        let child = cb.seal().unwrap();

        assert!(matches!(
            verify_chain(&child, &store),
            Err(Error::Integrity {
                what: "provenance_edge",
                ..
            })
        ));
    }

    /// Back-compat (ADR-0058): a pre-ADR-0058 manifest carries `producer` as a bare string. Reading
    /// then re-serializing MUST reproduce the exact string (not a struct), so the seal over the old
    /// bytes still holds and the conformance corpus / DP01 archive are not broken.
    #[test]
    fn legacy_producer_string_round_trips_byte_identical() {
        let json = r#"{"tessera_version":"0.0.0","id":"blake3:x","id_inputs":{},"product":"recon","name":"n","description":"d","timestamp":"2024-01-01T00:00:00Z","producer":"tessera/0.0.0"}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&m.producer, Some(ProducerRef::Legacy(s)) if s == "tessera/0.0.0"),
            "a bare string must parse as Legacy, got {:?}",
            m.producer
        );
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(
            back["producer"],
            serde_json::json!("tessera/0.0.0"),
            "a legacy string must NOT be rewritten as a struct (would break the seal)"
        );
        // The accessors read it uniformly.
        let p = m.producer.unwrap();
        assert_eq!(p.tool(), "tessera");
        assert_eq!(p.version(), "0.0.0");

        // The load-bearing claim: a manifest SEALED with a legacy string producer still VERIFIES
        // after a JSON round-trip — i.e. the canonical bytes the seal is computed over are reproduced
        // byte-for-byte, so an existing sealed .tsra never fails integrity under the new reader.
        let mut sealed = Manifest::new("recon", "n", "d", TS);
        sealed.producer = Some(ProducerRef::Legacy("acme-daq/1.2".into()));
        sealed.manifest_hash = Some(sealed.compute_manifest_hash().unwrap());
        let reparsed = Manifest::from_json(&sealed.to_json().unwrap()).unwrap();
        reparsed
            .verify()
            .expect("legacy-producer seal must still verify after round-trip");
        assert!(
            matches!(&reparsed.producer, Some(ProducerRef::Legacy(s)) if s == "acme-daq/1.2"),
            "the legacy producer survives the round-trip unchanged"
        );
    }

    /// A newly-sealed product carries the structured producer (tessera stamps its own tool+version),
    /// serialized as a map.
    #[test]
    fn structured_producer_is_stamped_on_seal() {
        let m = ProductBuilder::new("recon", "n", "d", TS).seal().unwrap();
        match &m.producer {
            Some(ProducerRef::Structured(p)) => assert_eq!(p.tool, "tessera"),
            other => panic!("expected a structured producer, got {other:?}"),
        }
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["producer"]["tool"], "tessera");
    }

    /// ADR-0058 §5: identity inheritance is **schema-driven** — only fields the schema flags
    /// `inherit` flow from parent to child; a non-flagged field (a transform setting) does not; an
    /// explicit child value always wins; the first-class `study` grouping key flows when unset.
    #[test]
    fn inherit_identity_is_schema_driven_and_child_wins() {
        use crate::schema::{FieldSpec, ProductSchema};

        let mut parent = Manifest::new("listmode", "raw", "d", TS);
        parent.study = Some("DP01".into());
        parent
            .metadata
            .insert("patient_id".into(), serde_json::json!("ANON9297"));
        parent
            .metadata
            .insert("exam".into(), serde_json::json!("9297"));
        parent
            .metadata
            .insert("energy_window".into(), serde_json::json!("425-650")); // not inheritable

        let mut child = Manifest::new("listmode", "events", "d", TS);
        child
            .metadata
            .insert("patient_id".into(), serde_json::json!("KEEP")); // child override

        let schema = ProductSchema {
            product: "listmode".into(),
            version: "1".into(),
            description: String::new(),
            fields: vec![
                FieldSpec::optional("patient_id", "", "string").inheritable(),
                FieldSpec::optional("exam", "", "string").inheritable(),
                FieldSpec::optional("energy_window", "", "string"), // NOT inheritable
            ],
            blocks: Vec::new(),
            requires_generation: false,
        };

        inherit_identity(&mut child, &parent, &schema);

        assert_eq!(
            child.metadata.get("patient_id"),
            Some(&serde_json::json!("KEEP")),
            "an explicit child value wins over the parent's"
        );
        assert_eq!(
            child.metadata.get("exam"),
            Some(&serde_json::json!("9297")),
            "a schema-flagged field is inherited"
        );
        assert_eq!(
            child.metadata.get("energy_window"),
            None,
            "a non-inheritable field (a transform setting) does not flow down"
        );
        assert_eq!(
            child.study.as_deref(),
            Some("DP01"),
            "the study grouping key is inherited when unset"
        );
    }
}
