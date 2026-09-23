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
        // `producer` and `generation` are intentionally NOT carried: a new version is sealed by
        // *this* build (producer is re-stamped in `seal`), and its generation recipe is a property of
        // how *this* revision was made — the caller re-attaches one via `with_generation` if needed.
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

    /// Declare the producing tool/build (ADR-0058 §1) — an external DAQ/SIM/recon records its own
    /// identity here, overriding the default `tessera` stamp. Sealed provenance.
    pub fn with_producer(&mut self, producer: crate::provenance::Producer) -> &mut Self {
        self.manifest.producer = Some(crate::provenance::ProducerRef::Structured(producer));
        self
    }

    /// Attach the generation record (ADR-0058 §2) — *how* this product was made, as a generic bag
    /// (inline `config` and/or a `config_ref` to a carried block). Required at validate for schemas
    /// that set `requires_generation`.
    pub fn with_generation(&mut self, generation: crate::provenance::Generation) -> &mut Self {
        self.manifest.generation = Some(generation);
        self
    }

    /// Inherit **schema-flagged identity** fields from a resolved `derived_from` parent (ADR-0058
    /// §5) — the DAG-walk caller (the ingest engine) supplies the parent manifest + this product's
    /// schema; only fields the schema marks `inherit` flow, and an explicit child value always wins.
    /// Call before `seal` so the inherited identity is covered by the seal.
    pub fn inherit_identity_from(
        &mut self,
        parent: &Manifest,
        schema: &crate::schema::ProductSchema,
    ) -> &mut Self {
        crate::provenance::inherit_identity(&mut self.manifest, parent, schema);
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
        // Sealed provenance: stamp the producing tool/build so a reader knows what wrote the file
        // (ADR-0058 §1 — structured [`ProducerRef::tessera()`], tool + `TESSERA_VERSION` +
        // optional build commit). Re-stamped per version (not inherited) — a new version is sealed
        // by *this* tool.
        //
        // Note the seal keys off the **format version** (`TESSERA_VERSION`), not the software
        // (`CARGO_PKG_VERSION`) — identity-relevant and stable across software/crate version
        // bumps. The build-tool/software version is non-sealed provenance and lives in
        // `aux/provenance.json` (ADR-0042), so a `cargo` version bump never changes the seal or
        // forces a conformance-corpus regen. (`TESSERA_VERSION` only changes on a *deliberate*
        // format revision — where a regen is expected.)
        if self.manifest.producer.is_none() {
            self.manifest.producer = Some(crate::provenance::ProducerRef::tessera());
        }
        // The seal is computed last, over the manifest with `manifest_hash` excluded, so it
        // transitively commits to id_inputs, sources, the producer, the embedded schema, and blocks.
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

    /// ADR-0056 §6a: decoder drift is detectable from **sealed data alone**, even before the
    /// decoder identity is recorded.
    ///
    /// This is the property that makes the decoder identity a *recipe fact* — recorded in the
    /// sealed provenance bag under the well-known key `ingest_decoder` (ADR-0056 §6a), not a
    /// bespoke sealed field. The seal already pins both ends of the ingest transform — the input,
    /// via the `ingested_from` edge's source digest, and the output, via `content_hash` — so
    /// *same source digest + different content_hash* means the interpretation changed, and no
    /// dedicated field is needed to *detect* it. Recording the decoder is for *attribution*, not
    /// detection.
    ///
    /// If a refactor ever dropped the source digest from the edge, or folded product metadata into
    /// `content_hash`, that inference would break silently and #403's decision would lose its
    /// premise. Both halves are asserted here.
    #[test]
    fn the_sealed_source_and_content_hashes_bracket_the_ingest_transform() {
        // Same source file, decoded twice. `decoded` is the block digest, standing in for whatever
        // logical values the decoder extracted; `context` is a piece of recorded metadata — sealed,
        // but not part of `content_hash`.
        let sealed = |decoded: &str, context: &str| {
            let mut b = ProductBuilder::new("table", "trades", "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(block("data", decoded));
            b.add_source(
                Source::new("ingested_from", "trades.parquet").with_content_hash("blake3:src"),
            );
            b.with_field("source_format", serde_json::json!(context));
            b.seal().unwrap()
        };

        let same_values = sealed("blake3:aa", "parquet");
        let drifted = sealed("blake3:bb", "parquet");

        // The input is pinned inside the seal — this is what makes the comparison meaningful.
        let src = |m: &Manifest| {
            m.sources
                .iter()
                .find(|s| s.role == "ingested_from")
                .and_then(|s| s.content_hash.clone())
                .expect("ingest stamps a source digest")
        };
        assert_eq!(src(&same_values), src(&drifted), "same source file");

        // Same source + moved content_hash ⇒ the interpretation changed. Detection, from the seal.
        assert_ne!(
            same_values.content_hash, drifted.content_hash,
            "different extracted values must move content_hash"
        );
        assert_ne!(same_values.manifest_hash, drifted.manifest_hash);
        assert_eq!(
            same_values.id, drifted.id,
            "drift is a new version of one logical product, not a new product"
        );

        // The other half: a value-preserving change to recorded context moves `manifest_hash`
        // (metadata changed) but never `content_hash`/`id` — the data fingerprint tracks the
        // extracted *values*, not the recipe. That is exactly why recording the decoder (§6a) is a
        // sealed recipe fact that can never move `content_hash` on its own.
        let relabelled = sealed("blake3:aa", "parquet-v2");
        assert_eq!(
            same_values.content_hash, relabelled.content_hash,
            "content_hash is a Merkle over block digests; manifest metadata is not in it"
        );
        assert_eq!(same_values.id, relabelled.id);
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
