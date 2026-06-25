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

    /// Seal: roll block digests into a Merkle root, freeze the manifest, return it.
    pub fn seal(mut self) -> crate::Result<Manifest> {
        let digests: Vec<String> = self.refs.iter().filter_map(|r| r.digest.clone()).collect();
        self.manifest.blocks = self.refs;
        self.manifest.content_hash = Some(crate::hash::merkle_root(&digests));
        Ok(self.manifest)
    }
}
