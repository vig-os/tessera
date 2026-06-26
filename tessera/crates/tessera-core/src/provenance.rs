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
}
