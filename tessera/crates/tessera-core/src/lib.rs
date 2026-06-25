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
pub mod error;
pub mod hash;
pub mod identity;
pub mod manifest;
pub mod product;
pub mod provenance;

pub use error::{Error, Result};
pub use manifest::Manifest;
pub use product::ProductBuilder;

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
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(m.blocks[0].kind, BlockKind::Array);
        // identity is stable for identical inputs
        let again = Manifest::new("recon", "DP06-CT-Standard-3.75", "x", "2023-12-08T00:00:00+00:00");
        assert_eq!(m.id, again.id);
    }

    #[test]
    fn build_listmode_table_product() {
        let spec = TableSpec {
            columns: vec![
                Column { name: "lt".into(), dtype: "f4".into(), codec: Some("zstd".into()) },
                Column { name: "en0".into(), dtype: "f4".into(), codec: Some("zstd".into()) },
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
