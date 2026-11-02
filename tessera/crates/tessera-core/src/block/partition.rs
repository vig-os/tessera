//! Logical tables over numbered leaves (ADR-0064): the partition marker and the leaf naming.
//!
//! A table of two or more blocks is written as the leaves `<table>/0000`, `<table>/0001`, …, each
//! carrying a [`Partition`] marker in its sealed [`TableSpec`](super::table::TableSpec). The leaf of
//! index `i` is `format!("{i:04}")`: padded to four digits, never longer than needed (`10000` past
//! `9999`), and the manifest order is the index order. Products written before ADR-0064 name their
//! blocks `<table>_NNNN` without a marker; that **legacy flat** form resolves as it always has.
#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

/// A leaf's position in its partitioned table (ADR-0064 §2): set on every leaf, never on a single
/// block. Serialized as `"partition": {"index": 3, "last": false}`, both fields always present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Partition {
    /// The leaf's index: its name is `<table>/{index:04}` ([`leaf_name`]).
    pub index: u64,
    /// True on exactly one leaf, the one with the highest index.
    pub last: bool,
}

/// The canonical leaf segment of index `index`: `format!("{index:04}")` (ADR-0064 §1).
pub fn leaf_segment(index: u64) -> String {
    format!("{index:04}")
}

/// The canonical name of leaf `index` of the partitioned table `table`: `<table>/{index:04}`.
pub fn leaf_name(table: &str, index: u64) -> String {
    format!("{table}/{}", leaf_segment(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::table::Column;
    use crate::block::table::TableSpec;
    use crate::block::{BlockKind, BlockRef};
    use crate::ProductBuilder;
    use serde_json::json;

    fn spec(cols: &[(&str, &str)], partition: Option<Partition>) -> serde_json::Value {
        let columns = cols.iter().map(|(n, d)| Column::new(*n, *d)).collect();
        let s = TableSpec {
            columns,
            rows: 2,
            row_index: None,
            partition,
        };
        serde_json::to_value(s).unwrap()
    }

    fn block(name: &str, kind: BlockKind, spec: serde_json::Value) -> BlockRef {
        BlockRef {
            name: name.parse().unwrap(),
            kind,
            digest: Some(crate::hash::digest(name.as_bytes())),
            spec,
        }
    }

    /// A table block, marked with `(index, last)` when given.
    fn table(name: &str, marker: Option<(u64, bool)>) -> BlockRef {
        let p = marker.map(|(index, last)| Partition { index, last });
        block(name, BlockKind::Table, spec(&[("t", "u8")], p))
    }

    /// The valid partition `pairs/0000..=pairs/{n-1}`.
    fn leaves(n: u64) -> Vec<BlockRef> {
        (0..n)
            .map(|i| table(&leaf_name("pairs", i), Some((i, i + 1 == n))))
            .collect()
    }

    fn seal(blocks: Vec<BlockRef>) -> crate::Result<crate::Manifest> {
        let mut b = ProductBuilder::new("listmode", "p", "d", "2024-01-01T00:00:00Z");
        for r in blocks {
            b.add_block_ref(r);
        }
        b.seal()
    }

    #[test]
    fn leaf_names_pad_per_index_and_never_overflow() {
        assert_eq!(leaf_name("pairs", 3), "pairs/0003");
        assert_eq!(leaf_name("pairs", 9999), "pairs/9999");
        assert_eq!(leaf_name("pairs", 10000), "pairs/10000");
        assert_eq!(leaf_segment(u64::MAX), u64::MAX.to_string());
    }

    #[test]
    fn marker_serializes_both_fields_and_single_block_spec_is_unchanged() {
        let marked = spec(
            &[("t", "u8")],
            Some(Partition {
                index: 3,
                last: false,
            }),
        );
        assert_eq!(marked["partition"], json!({"index": 3, "last": false}));
        let back: TableSpec = serde_json::from_value(marked).unwrap();
        assert_eq!(
            back.partition,
            Some(Partition {
                index: 3,
                last: false
            })
        );
        // A single-block spec emits no `partition` key: corpus bytes and hashes are untouched.
        // Compared as JSON values, not strings: key order follows `serde_json`'s `preserve_order`,
        // which workspace feature unification turns on (see `table.rs`'s `json_keys`); the seal
        // canonicalises through JCS either way.
        assert_eq!(
            spec(&[("t", "u8")], None),
            json!({"columns": [{"dtype": "u8", "name": "t"}], "rows": 2})
        );
    }

    /// ADR-0064 §6: `manifest_hash` binds the leaf names and the marker; `content_hash` does not.
    #[test]
    fn manifest_hash_binds_names_and_marker_content_hash_does_not() {
        let digests_of = |m: &crate::Manifest| m.content_hash.clone().unwrap();
        let base = seal(leaves(2)).unwrap();
        // Same digests, the marker dropped (an unmarked node seals: nothing claims a partition).
        let mut unmarked = leaves(2);
        for b in &mut unmarked {
            b.spec.as_object_mut().unwrap().remove("partition");
        }
        // Same digests and markers, the table renamed.
        let mut renamed = leaves(2);
        for (i, b) in renamed.iter_mut().enumerate() {
            b.name = leaf_name("rows", i as u64).parse().unwrap();
        }
        for other in [seal(unmarked).unwrap(), seal(renamed).unwrap()] {
            assert_ne!(other.manifest_hash, base.manifest_hash);
            assert_eq!(digests_of(&other), digests_of(&base));
        }
        // Deterministic: the same partition seals the same manifest.
        assert_eq!(seal(leaves(2)).unwrap().manifest_hash, base.manifest_hash);
    }
}
