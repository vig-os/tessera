//! A [`ProductBuilder`] assembles a manifest + blocks and seals it with a Merkle hash.
//!
//! Build → add blocks → `seal()`. Sealing computes the content hash and freezes the
//! manifest (products are immutable; a new version is a new product with a `sources` edge
//! to its parent — see `docs/rfc-tessera.md` §3, and fd5 audit-trail issues #167–170).

use crate::block::{Block, BlockRef};
use crate::manifest::Manifest;
use crate::provenance::Source;

pub struct ProductBuilder {
    manifest: Manifest,
    refs: Vec<BlockRef>,
}

impl ProductBuilder {
    pub fn new(
        product: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        ProductBuilder {
            manifest: Manifest::new(product, name, description, timestamp),
            refs: Vec::new(),
        }
    }

    /// Start a **new version** from an existing sealed manifest (ADR-0036 `evolve`): pre-loads the
    /// parent's identity (`product`/`name`/`timestamp` → the **same `id`**, since `id` is the stable
    /// lineage handle), its blocks, metadata, study, schema, and its *derivation* provenance edges.
    /// The parent's own `supersedes` edges are **dropped** — a version carries exactly one edge to its
    /// immediate parent (the chain is walked, not accumulated); the caller adds it (typically
    /// `add_source(Source::new("supersedes", parent.manifest_hash).with_content_hash(parent.manifest_hash))`).
    /// Apply the delta (`with_field` / `add_block` / `add_source`), then [`seal`](Self::seal) — which
    /// recomputes `content_hash` + `manifest_hash` while `id` stays put. Unchanged blocks keep their
    /// digests, so a metadata-only version re-stores only the new manifest object.
    pub fn from_manifest(parent: &Manifest) -> Self {
        let mut manifest = Manifest::new(
            &parent.product,
            &parent.name,
            &parent.description,
            &parent.timestamp,
        );
        manifest.study = parent.study.clone();
        manifest.schema = parent.schema.clone();
        manifest.metadata = parent.metadata.clone();
        manifest.extra = parent.extra.clone();
        // Keep derivation/provenance edges; drop the parent's version edges (walked, not accumulated).
        manifest.sources = parent
            .sources
            .iter()
            .filter(|s| s.role != "supersedes")
            .cloned()
            .collect();
        ProductBuilder {
            manifest,
            refs: parent.blocks.clone(),
        }
    }

    /// Add a storage block (array or table); records its manifest reference.
    pub fn add_block(&mut self, block: &dyn Block) -> crate::Result<&mut Self> {
        self.refs.push(block.block_ref()?);
        Ok(self)
    }

    /// Add a precomputed block reference (e.g. from a backend that already wrote + digested
    /// its payload). The ref must carry a digest, or [`seal`](Self::seal) will reject it.
    pub fn add_block_ref(&mut self, block_ref: BlockRef) -> &mut Self {
        self.refs.push(block_ref);
        self
    }

    /// Remove a block reference by name (used by `commit --remove-block`). Returns `true` if one was
    /// removed. This is a manifest edit only — the block's stored object is untouched, since other
    /// versions may still reference it (a repository `gc` reclaims unreachable objects later).
    pub fn remove_block(&mut self, name: &str) -> bool {
        let before = self.refs.len();
        self.refs.retain(|r| r.name != name);
        self.refs.len() != before
    }

    /// Record a provenance edge to an upstream artifact.
    pub fn add_source(&mut self, source: Source) -> &mut Self {
        self.manifest.sources.push(source);
        self
    }

    /// Set the study/grouping id (ties this product to the others of the same exam).
    pub fn with_study(&mut self, study: impl Into<String>) -> &mut Self {
        self.manifest.study = Some(study.into());
        self
    }

    /// Set a schema-defined metadata field value (keyed by the field's stable `id`).
    pub fn with_field(&mut self, id: impl Into<String>, value: serde_json::Value) -> &mut Self {
        self.manifest.metadata.insert(id.into(), value);
        self
    }

    /// Add an extension (`extra/`) field — non-standard / vendor metadata, preserved + hashed
    /// but not schema-validated.
    pub fn with_extra(&mut self, key: impl Into<String>, value: serde_json::Value) -> &mut Self {
        self.manifest.extra.insert(key.into(), value);
        self
    }

    /// Attach the product's embedded JSON Schema.
    pub fn with_schema(&mut self, schema: serde_json::Value) -> &mut Self {
        self.manifest.schema = Some(schema);
        self
    }

    /// Seal: roll block digests into the content Merkle root, then hash the whole manifest into
    /// the `manifest_hash` seal, freeze, and return it.
    ///
    /// Every block MUST carry a digest — a missing digest is a hard error, never silently
    /// dropped (otherwise a block would be invisible to the content hash yet present in the
    /// manifest, so two different products could hash identically).
    pub fn seal(mut self) -> crate::Result<Manifest> {
        let mut digests = Vec::with_capacity(self.refs.len());
        for r in &self.refs {
            match &r.digest {
                Some(d) => digests.push(d.clone()),
                None => return Err(crate::Error::MissingDigest(r.name.clone())),
            }
        }
        self.manifest.blocks = self.refs;
        self.manifest.content_hash = Some(crate::hash::merkle_root(&digests));
        // Self-describing (FAIR-Reusable): embed the resolved product schema so the sealed `.tsra`
        // carries its own contract and the seal commits to it. A caller-supplied schema
        // (`with_schema`) or an inherited one (`from_manifest`) wins; otherwise embed the built-in
        // registry's schema for a known product. Open-world products (unknown to the registry) have
        // no schema to embed — the field stays absent (the permissive escape hatch).
        if self.manifest.schema.is_none() {
            if let Some(s) = crate::schema::SchemaRegistry::builtin().get(&self.manifest.product) {
                self.manifest.schema = Some(s.to_value()?);
            }
        }
        // The seal is computed last, over the manifest with `manifest_hash` excluded, so it
        // transitively commits to id_inputs, sources, the embedded schema, and every block digest.
        self.manifest.manifest_hash = None;
        self.manifest.manifest_hash = Some(self.manifest.compute_manifest_hash()?);
        Ok(self.manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockKind, BlockRef};

    fn block(name: &str, digest: &str) -> BlockRef {
        BlockRef {
            name: name.into(),
            kind: BlockKind::Array,
            digest: Some(digest.into()),
            spec: serde_json::json!({ "dtype": "int16", "shape": [2] }),
        }
    }

    #[test]
    fn from_manifest_keeps_id_drops_old_supersedes_changes_seal() {
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block("v", "blake3:aa"));
        b.with_field("tracer", serde_json::json!("FDG"));
        b.add_source(Source::new("ingested_from", "x.dcm").with_content_hash("blake3:src"));
        b.add_source(Source::new("supersedes", "blake3:old").with_content_hash("blake3:old"));
        let v1 = b.seal().unwrap();
        let v1mh = v1.manifest_hash.clone().unwrap();

        // Evolve: change one metadata field, stamp the supersedes edge to the immediate parent.
        let mut e = ProductBuilder::from_manifest(&v1);
        e.with_field("tracer", serde_json::json!("FLT"));
        e.add_source(Source::new("supersedes", &v1mh).with_content_hash(&v1mh));
        let v2 = e.seal().unwrap();

        assert_eq!(v1.id, v2.id, "id is the stable lineage handle (model A)");
        assert_ne!(v1.manifest_hash, v2.manifest_hash, "new version → new seal");
        assert_eq!(
            v1.content_hash, v2.content_hash,
            "same blocks → same content_hash"
        );
        assert_eq!(v2.metadata.get("tracer"), Some(&serde_json::json!("FLT")));
        assert!(
            v2.sources.iter().any(|s| s.role == "ingested_from"),
            "derivation edges are kept"
        );
        let sup: Vec<_> = v2
            .sources
            .iter()
            .filter(|s| s.role == "supersedes")
            .collect();
        assert_eq!(
            sup.len(),
            1,
            "exactly one supersedes edge (parent's dropped, ours added)"
        );
        assert_eq!(
            sup[0].reference, v1mh,
            "points at the immediate parent version"
        );
    }

    #[test]
    fn seal_embeds_the_product_schema_for_a_known_product() {
        // A known product's sealed manifest carries its own contract (self-describing, obligatory).
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block("v", "blake3:aa"));
        let m = b.seal().unwrap();

        let embedded = m.schema.as_ref().expect("known product embeds its schema");
        let parsed = crate::schema::ProductSchema::from_value(embedded).unwrap();
        let registry = crate::SchemaRegistry::builtin()
            .get("recon")
            .unwrap()
            .clone();
        assert_eq!(
            parsed, registry,
            "embedded schema == the registry schema it resolved"
        );
        // Validation routes through the embedded copy; equivalent to the registry path (same
        // version). This minimal fixture omits required recon fields, so both agree it is NOT valid.
        assert_eq!(
            crate::validate_manifest(&m).is_ok(),
            crate::SchemaRegistry::builtin().validate(&m).is_ok(),
            "embedded-schema validation matches the registry path"
        );
        // The embedded schema is inside the seal → it is covered by manifest_hash.
        assert!(m.manifest_hash.is_some());
    }

    #[test]
    fn seal_leaves_open_world_products_schema_free() {
        // An unknown (extension) product has no registry schema to embed — the field stays absent,
        // the permissive open-world escape hatch, and validation is permissive.
        let mut b = ProductBuilder::new("acme-custom-product", "X", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block("v", "blake3:aa"));
        let m = b.seal().unwrap();
        assert!(m.schema.is_none(), "open-world product embeds no schema");
        crate::validate_manifest(&m).unwrap();
    }
}
