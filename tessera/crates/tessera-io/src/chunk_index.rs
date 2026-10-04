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
pub fn chunk_index_block(
    data_name: &str,
    data_digest: &str,
    index: &ChunkIndex,
) -> Result<(BlockRef, BlockPayload)> {
    let name = cidx_name(data_name);
    let payload = index.to_bytes()?;
    let dg = digest(&payload);
    let mut spec = serde_json::json!({
        // ADR-0028 §4 derived-sidecar tag: this block is **regenerable** from the data block it
        // indexes (via table_chunk_index / array_chunk_index), so it is `class: "derived"` with a
        // versioned `recipe`. A consumer may drop + rebuild it; it is not canonical source data.
        "class": "derived",
        // `@2` = the ADR-0059 schema: native-LE per-chunk digests (M3), the S2 counters, an optional
        // block histogram, and `indexed_digest` (M1). A reader must not assume a `@1` index carries
        // any of them, and must not read a `@2` digest as if it were a `@1` one.
        "recipe": "chunk_index@2",
        "indexes": data_name,        // the data block this is the chunk-index of
        // ADR-0059 M1. The index was bound to its block only by NAME, which cannot be checked: a
        // `commit --remove-block volume --add-block other:volume` left the old `volume.cidx` in
        // place and `stats` served the OLD block's numbers labelled exact. Recording the digest the
        // index was built against makes the binding verifiable — a reader compares it to the block's
        // own `BlockRef.digest` and refuses on a mismatch.
        "indexed_digest": data_digest,
        "entries": index.len(),       // number of sub-block entries
        "root": index.root(),         // sub-block Merkle (MMR) root, ADR-0028 §1
    });
    // Self-describe the histogram's edges so a reader never has to guess them (ADR-0059 §5), under
    // the ratified key `hist`. Omitted when there is none: absent and all-zero must not be
    // confusable.
    //
    // This duplicates edges the PAYLOAD also carries, deliberately and with a stated authority: the
    // payload is AUTHORITATIVE (it is the content-hashed block, and `counts` is meaningless without
    // its edges, so the block must be interpretable without the manifest), while this descriptor
    // lets `inspect` report the histogram's shape without reading the block. Both are written from
    // `index.histogram` by the one expression below, so they cannot diverge at write time, and a
    // hand-edited manifest fails the content hash. ADR-0059 §5 records the rule.
    if let Some(h) = &index.histogram {
        spec["hist"] = serde_json::json!({
            "kind": h.kind,
            "lo": h.lo,
            "hi": h.hi,
            "bins": h.counts.len(),
            "exact": h.exact,
        });
    }
    let block_ref = BlockRef {
        name: name.clone(),
        kind: BlockKind::ChunkIndex,
        digest: Some(dg),
        spec,
    };
    Ok((block_ref, BlockPayload::new(name, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::ProductBuilder;

    fn sample_index() -> ChunkIndex {
        let mut idx = ChunkIndex::new();
        assert!(idx.push(digest(b"chunk-0"), &[1, 2, 3]));
        assert!(idx.push(digest(b"chunk-1"), &[10, 20]));
        idx
    }

    #[test]
    fn block_digests_payload_and_payload_roundtrips() {
        let idx = sample_index();
        let (br, payload) = chunk_index_block("volume", "blake3:test-data-digest", &idx).unwrap();
        assert_eq!(br.name, "volume.cidx");
        assert_eq!(br.kind, BlockKind::ChunkIndex);
        // digest is over the exact payload bytes
        assert_eq!(br.digest.as_deref(), Some(digest(&payload.bytes).as_str()));
        // spec self-describes the index + carries the ADR-0028 §4 derived-sidecar tag
        assert_eq!(br.spec["indexes"], "volume");
        assert_eq!(br.spec["root"], idx.root());
        assert_eq!(br.spec["class"], "derived");
        assert_eq!(br.spec["recipe"], "chunk_index@2");
        // M1: the index records the digest of the block it was built against, so the binding is
        // checkable rather than trusted by name.
        assert_eq!(br.spec["indexed_digest"], "blake3:test-data-digest");
        // the payload reconstructs the index (same root + entries)
        let back = ChunkIndex::from_bytes(&payload.bytes).unwrap();
        assert_eq!(back.root(), idx.root());
        assert_eq!(back.entries, idx.entries);
    }

    #[test]
    fn block_rolls_into_a_sealed_product_and_verifies() {
        let (br, _payload) =
            chunk_index_block("volume", "blake3:test-data-digest", &sample_index()).unwrap();
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
