//! Emit a [`ChunkIndex`](tessera_core::chunk_index::ChunkIndex) (ADR-0028 §3) as an **additive
//! companion block**. The chunk-index for a data block `<name>` is packed as a sibling block
//! `<name>.cidx` whose payload is the index's deterministic [`ChunkIndex::to_bytes`] bytes and whose
//! `BlockRef.digest` is `digest(payload)` — exactly like any other block, so the seal
//! (`manifest_hash`) covers it. The block ref is [`BlockClass::Derived`] (ADR-0028 §4): it is a pure
//! function of the data block it indexes, so it is **not** part of the product's `content_hash`.
//!
//! It is **additive**: emitting (or not emitting) a chunk-index leaves the indexed data block's own
//! digest and the product's `content_hash` untouched, so existing products without a chunk-index are
//! unaffected (no corpus regeneration), and adding or regenerating one moves only `manifest_hash`.
//! A consumer that wants per-chunk verification or pruning reads the `.cidx` block; one that doesn't,
//! ignores it. The block's `spec` records the index `root` (the sub-block Merkle root, ADR-0028 §1) and
//! its entry count for self-description.

use tessera_core::block::{BlockClass, BlockKind, BlockName, BlockRef};
use tessera_core::chunk_index::ChunkIndex;
use tessera_core::hash::digest;
use tessera_core::Result;

use crate::BlockPayload;

/// The conventional sibling name for the chunk-index of a data block named `data_name`
/// (`<data_name>.cidx`): [`tessera_core::block::cidx_name`], the one rule the cascade on removal
/// ([`tessera_core::block::derived_family`]) uses too.
pub fn cidx_name(data_name: &str) -> String {
    tessera_core::block::cidx_name(data_name)
}

/// Build the additive chunk-index companion block for `data_name` from its computed [`ChunkIndex`]
/// (see [`crate::table::table_chunk_index`] / [`crate::array::array_chunk_index`]). Returns the
/// [`BlockRef`] (kind [`BlockKind::ChunkIndex`], class [`BlockClass::Derived`], digest over the
/// payload) and the [`BlockPayload`] to pack. Add the returned `BlockRef` to the product alongside the
/// data block: the seal covers the index's bytes, and `content_hash` does not (ADR-0028 §4).
pub fn chunk_index_block(data_name: &str, index: &ChunkIndex) -> Result<(BlockRef, BlockPayload)> {
    let name = BlockName::new(cidx_name(data_name))?;
    let payload = index.to_bytes()?;
    let dg = digest(&payload);
    let block_ref = BlockRef {
        name: name.clone(),
        kind: BlockKind::ChunkIndex,
        digest: Some(dg),
        // ADR-0028 §4 derived-sidecar class: this block is **regenerable** from the data block it
        // indexes (via table_chunk_index / array_chunk_index), so the block ref is `class: "derived"`
        // (it leaves `content_hash`) and the spec carries a versioned `recipe`. A consumer may drop +
        // rebuild it; it is not source data.
        class: BlockClass::Derived,
        spec: serde_json::json!({
            // Still @1 after #632, which took values under a NULL out of the table row-group digest.
            // @1 was always a pure function of the indexed block's logical content (ADR-0028 §4), and
            // the block never stores those bytes, so a root that moved with them was a defect, not a
            // different recipe. Every canonical producer (all ingest, ADR-0056 H5) is byte-unchanged.
            // The @2 bump is reserved for ADR-0059's schema event (§11 step 1: M1-M3 + new stats).
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
        // the ref carries the ADR-0028 §4 derived class (on the ref, once: not in the spec too), and
        // the spec self-describes the index
        assert_eq!(br.class, BlockClass::Derived);
        assert!(br.spec.get("class").is_none());
        assert_eq!(br.spec["indexes"], "volume");
        assert_eq!(br.spec["root"], idx.root());
        assert_eq!(br.spec["recipe"], "chunk_index@1");
        // the payload reconstructs the index (same root + entries)
        let back = ChunkIndex::from_bytes(&payload.bytes).unwrap();
        assert_eq!(back.root(), idx.root());
        assert_eq!(back.entries, idx.entries);
    }

    /// The seal of a `recon` product of `blocks`, as (`content_hash`, `manifest_hash`).
    fn hashes(blocks: &[BlockRef]) -> (String, String) {
        let mut b = ProductBuilder::new("recon", "p", "d", "2024-01-01T00:00:00Z");
        for r in blocks {
            b.add_block_ref(r.clone());
        }
        let m = b.seal().unwrap();
        assert!(m.verify().is_ok());
        (m.content_hash.unwrap(), m.manifest_hash.unwrap())
    }

    /// ADR-0028 §4 / ADR-0066 rule 12 (#720): a derived block is not part of `content_hash`, and the
    /// seal still commits to it. Adding a chunk index, or regenerating it with other bytes, leaves
    /// `content_hash` alone and moves `manifest_hash`; changing the primary block moves both.
    #[test]
    fn a_derived_block_moves_the_seal_and_not_content_hash() {
        use crate::array::{array_block, ArrayData};
        use tessera_core::block::array::ArraySpec;
        let spec = ArraySpec::new(vec![4, 4, 4], "int16");
        let volume = |n: i16| {
            let data = ArrayData::I16((0..64).map(|k| k as i16 + n).collect());
            array_block("volume", &spec, &data).unwrap().0
        };
        let (cidx, _) = chunk_index_block("volume", &sample_index()).unwrap();
        let mut other = sample_index();
        other.push(digest(b"chunk-2"), &[7]);
        let (regenerated, _) = chunk_index_block("volume", &other).unwrap();
        assert_ne!(
            cidx.digest, regenerated.digest,
            "precondition: other index bytes"
        );

        let (ch, mh) = hashes(&[volume(0)]);
        let (ch_added, mh_added) = hashes(&[volume(0), cidx.clone()]);
        assert_eq!(
            ch_added, ch,
            "adding a derived block leaves content_hash unchanged"
        );
        assert_ne!(mh_added, mh, "adding a derived block changes manifest_hash");
        let (ch_regen, mh_regen) = hashes(&[volume(0), regenerated]);
        assert_eq!(
            ch_regen, ch,
            "regenerating a derived block leaves content_hash unchanged"
        );
        assert_ne!(
            mh_regen, mh_added,
            "regenerating a derived block changes manifest_hash"
        );
        let (ch_primary, mh_primary) = hashes(&[volume(1), cidx]);
        assert_ne!(
            ch_primary, ch,
            "changing a primary block changes content_hash"
        );
        assert_ne!(
            mh_primary, mh_added,
            "changing a primary block changes manifest_hash"
        );
    }

    /// A product whose blocks are all derived has the `content_hash` of the empty set, the one
    /// `empty_no_blocks` carries; the derived block is still in the manifest, under the seal.
    #[test]
    fn an_all_derived_product_has_the_empty_content_hash() {
        let (br, _payload) = chunk_index_block("volume", &sample_index()).unwrap();
        let (ch, mh) = hashes(&[br]);
        let (empty_ch, empty_mh) = hashes(&[]);
        assert_eq!(ch, tessera_core::hash::merkle_root(&[]));
        assert_eq!(ch, empty_ch);
        assert_ne!(mh, empty_mh, "the seal covers the derived block");
    }
}
