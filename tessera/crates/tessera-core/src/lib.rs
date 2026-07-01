//! # Tessera — a substrate-agnostic FAIR data product format (spike)
//!
//! A Tessera **product** is one immutable, content-hashed, self-describing FAIR data
//! artifact = a [`manifest::Manifest`] spine + shape-dispatched storage [`block`]s
//! (N-D chunked arrays via zarrs, columnar tables via arrow/parquet).
//!
//! This crate is the *spike* skeleton: the manifest/identity/hash/provenance spine and the
//! block descriptors are real; the block *payload* read/write paths are stubs behind the
//! `array-zarr` / `table-arrow` features. See `docs/rfc-tessera.md` for design + benchmark
//! rationale, and fd5 issues #192/#193/#194 for the evidence.

pub mod block;
pub mod canonical;
pub mod chunk_index;
pub mod collection;
pub mod dtype;
pub mod error;
pub mod export;
pub mod hash;
pub mod identity;
pub mod manifest;
pub mod product;
pub mod provenance;
pub mod referencing;
pub mod schema;
pub mod signing;

pub use collection::{Collection, CollectionBuilder, CollectionMember, Role};
pub use error::{Error, Result};
pub use manifest::Manifest;
pub use product::ProductBuilder;
pub use schema::{validate_manifest, ProductSchema, SchemaRegistry};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::array::{ArrayBlock, ArraySpec};
    use crate::block::table::{Column, TableBlock, TableSpec};
    use crate::block::BlockKind;

    #[test]
    fn build_and_seal_recon_product() {
        // A recon product: one native-int16 volume array block (no float32 upcast).
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![487, 512, 512], "int16"));
        let mut b = ProductBuilder::new(
            "recon",
            "DP06-CT-Standard-3.75",
            "DUPLET DP06 CT Standard 3.75mm",
            "2023-12-08T00:00:00+00:00",
        );
        b.add_block(&vol).unwrap();
        let m = b.seal().unwrap();
        assert!(m.id.starts_with("blake3:"));
        assert!(m.content_hash.is_some());
        assert!(m.manifest_hash.is_some());
        assert!(m.is_sealed());
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(m.blocks[0].kind, BlockKind::Array);
        // a freshly sealed product verifies all three hashes
        m.verify().unwrap();
        // identity is stable for identical id_inputs (description is NOT identity-bearing)
        let again = Manifest::new(
            "recon",
            "DP06-CT-Standard-3.75",
            "different description",
            "2023-12-08T00:00:00+00:00",
        );
        assert_eq!(m.id, again.id);
    }

    #[test]
    fn seal_round_trips_through_canonical_json_and_verifies() {
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        let mut b = ProductBuilder::new("recon", "rt", "round-trip", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        let m = b.seal().unwrap();
        // Persisted as JSON, re-read, the seal must reproduce byte-for-byte and verify.
        let parsed = Manifest::from_json_verified(&m.to_json().unwrap()).unwrap();
        assert_eq!(parsed.manifest_hash, m.manifest_hash);
        assert_eq!(
            parsed.compute_manifest_hash().unwrap(),
            m.manifest_hash.unwrap()
        );
    }

    #[test]
    fn tampering_with_a_block_digest_fails_verification() {
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        let mut b = ProductBuilder::new("recon", "t", "tamper", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        let mut m = b.seal().unwrap();
        m.blocks[0].digest = Some("blake3:deadbeef".into());
        match m.verify() {
            Err(crate::Error::Integrity { what, .. }) => assert_eq!(what, "content_hash"),
            other => panic!("expected content_hash integrity error, got {other:?}"),
        }
    }

    #[test]
    fn consistency_proof_links_an_append_only_product_revision() {
        // A proof fixture at the manifest level (ADR-0028 §2 + ADR-0022 versioning): one product
        // revision is an append-only extension of a prior one iff its content_hash is a proven MMR
        // prefix of the new content_hash.
        use crate::hash::{consistency_proof, verify_consistency};
        let blocks: Vec<ArrayBlock> = (0..5)
            .map(|i| {
                ArrayBlock::new(
                    format!("b{i}"),
                    ArraySpec::new(vec![8 + i as u64, 8, 8], "int16"),
                )
            })
            .collect();
        let seal = |k: usize| {
            let mut b = ProductBuilder::new(
                "recon",
                "rev",
                "append-only revision",
                "2024-01-01T00:00:00Z",
            );
            for blk in &blocks[..k] {
                b.add_block(blk).unwrap();
            }
            b.seal().unwrap()
        };
        let v1 = seal(3); // earlier revision: first three blocks
        let v2 = seal(5); // later revision: the same three + two appended

        // the ordered block digests of v2; v1's are the length-3 prefix (same blocks, same order).
        let digests: Vec<String> = v2
            .blocks
            .iter()
            .map(|r| r.digest.clone().unwrap())
            .collect();
        let proof = consistency_proof(&digests, 3).unwrap();
        assert!(
            verify_consistency(
                &proof,
                v1.content_hash.as_deref().unwrap(),
                v2.content_hash.as_deref().unwrap(),
            ),
            "v1 must be a proven append-only prefix of v2"
        );
        // a divergent product (different first block) is NOT a consistent prefix of v2.
        let mut other = ProductBuilder::new("recon", "rev", "divergent", "2024-01-01T00:00:00Z");
        other
            .add_block(&ArrayBlock::new(
                "different",
                ArraySpec::new(vec![9, 9, 9], "int16"),
            ))
            .unwrap();
        let od = other.seal().unwrap();
        assert!(!verify_consistency(
            &proof,
            od.content_hash.as_deref().unwrap(),
            v2.content_hash.as_deref().unwrap(),
        ));
    }

    #[test]
    fn build_listmode_table_product() {
        let spec = TableSpec {
            columns: vec![
                Column {
                    name: "lt".into(),
                    dtype: "f4".into(),
                    codec: Some("zstd".into()),
                },
                Column {
                    name: "en0".into(),
                    dtype: "f4".into(),
                    codec: Some("zstd".into()),
                },
            ],
            rows: 2_696_935,
            row_index: Some("ms".into()),
        };
        let events = TableBlock::new("events_3p", spec);
        let mut b = ProductBuilder::new(
            "listmode",
            "DP06-abdomen",
            "extended-coinc 3-photon events",
            "2023-12-08T00:00:00+00:00",
        );
        b.add_block(&events).unwrap();
        let m = b.seal().unwrap();
        assert_eq!(m.blocks[0].kind, BlockKind::Table);
        assert!(m.to_json().unwrap().contains("events_3p"));
    }
}
