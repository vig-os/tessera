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

    /// Record a provenance edge to an upstream artifact.
    pub fn add_source(&mut self, source: Source) -> &mut Self {
        self.manifest.sources.push(source);
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
        // The seal is computed last, over the manifest with `manifest_hash` excluded, so it
        // transitively commits to id_inputs, sources, and every block digest.
        self.manifest.manifest_hash = None;
        self.manifest.manifest_hash = Some(self.manifest.compute_manifest_hash()?);
        Ok(self.manifest)
    }
}
