//! Emit a [`ChunkIndex`](tessera_core::chunk_index::ChunkIndex) (ADR-0028 §3) as an **additive
//! companion block**. The chunk-index for a data block `<name>` is packed as a sibling block
//! `<name>.cidx` whose payload is the index's deterministic [`ChunkIndex::to_bytes`] bytes and whose
//! `BlockRef.digest` is `digest(payload)` — exactly like any other block, so it rolls into the product
//! content hash at seal time.
//!
//! It is **additive**: emitting (or not emitting) a chunk-index leaves the indexed data block's own
//! digest untouched, so existing products without a chunk-index are unaffected (no corpus regeneration).
//! A consumer that wants per-chunk verification or pruning reads the `.cidx` block; one that doesn't,
//! ignores it. The block's `spec` records the index `root` (the sub-block Merkle root, ADR-0028 §1) and
//! its entry count for self-description.

use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::chunk_index::ChunkIndex;
use tessera_core::hash::digest;
use tessera_core::Result;

use crate::BlockPayload;

/// The conventional sibling name for the chunk-index of a data block named `data_name`.
pub fn cidx_name(data_name: &str) -> String {
    format!("{data_name}.cidx")
}

/// Build the additive chunk-index companion block for `data_name` from its computed [`ChunkIndex`]
/// (see [`crate::table::table_chunk_index`] / [`crate::array::array_chunk_index`]). Returns the
/// [`BlockRef`] (kind [`BlockKind::ChunkIndex`], digest over the payload) and the [`BlockPayload`] to
/// pack. Add the returned `BlockRef` to the product alongside the data block; the index's bytes roll
/// into the content hash like any block.
pub fn chunk_index_block(data_name: &str, index: &ChunkIndex) -> Result<(BlockRef, BlockPayload)> {
    let name = cidx_name(data_name);
    let payload = index.to_bytes()?;
    let dg = digest(&payload);
    let block_ref = BlockRef {
        name: name.clone(),
        kind: BlockKind::ChunkIndex,
        digest: Some(dg),
        spec: serde_json::json!({
            // ADR-0028 §4 derived-sidecar tag: this block is **regenerable** from the data block it
            // indexes (via table_chunk_index / array_chunk_index), so it is `class: "derived"` with a
            // versioned `recipe`. A consumer may drop + rebuild it; it is not canonical source data.
            "class": "derived",
            "recipe": "chunk_index@1",
            "indexes": data_name,        // the data block this is the chunk-index of
            "entries": index.len(),       // number of sub-block entries
            "root": index.root(),         // sub-block Merkle (MMR) root, ADR-0028 §1
        }),
    };
    Ok((block_ref, BlockPayload::new(name, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::ProductBuilder;

    fn sample_index() -> ChunkIndex {
        let mut idx = ChunkIndex::new();
        idx.push(digest(b"chunk-0"), &[1, 2, 3]);
        idx.push(digest(b"chunk-1"), &[10, 20]);
        idx
    }

    #[test]
    fn block_digests_payload_and_payload_roundtrips() {
        let idx = sample_index();
        let (br, payload) = chunk_index_block("volume", &idx).unwrap();
        assert_eq!(br.name, "volume.cidx");
        assert_eq!(br.kind, BlockKind::ChunkIndex);
        // digest is over the exact payload bytes
        assert_eq!(br.digest.as_deref(), Some(digest(&payload.bytes).as_str()));
        // spec self-describes the index + carries the ADR-0028 §4 derived-sidecar tag
        assert_eq!(br.spec["indexes"], "volume");
        assert_eq!(br.spec["root"], idx.root());
        assert_eq!(br.spec["class"], "derived");
        assert_eq!(br.spec["recipe"], "chunk_index@1");
        // the payload reconstructs the index (same root + entries)
        let back = ChunkIndex::from_bytes(&payload.bytes).unwrap();
        assert_eq!(back.root(), idx.root());
        assert_eq!(back.entries, idx.entries);
    }

    #[test]
    fn block_rolls_into_a_sealed_product_and_verifies() {
        let (br, _payload) = chunk_index_block("volume", &sample_index()).unwrap();
        let mut b = ProductBuilder::new("recon", "p", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
        let m = b.seal().unwrap();
        // the chunk-index block is a first-class block: it's in the manifest and its digest rolled
        // into the content hash (so tampering with the index is detectable), and verify() passes.
        assert!(m.content_hash.is_some());
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(m.blocks[0].kind, BlockKind::ChunkIndex);
        assert!(m.verify().is_ok());
    }
}
