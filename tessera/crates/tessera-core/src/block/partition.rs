//! Logical tables over numbered leaves (ADR-0064): the partition marker, the closed list of child
//! classes a partitioned node may hold, and the one manifest-only function that resolves a table
//! name. Every reader and the seal go through [`table_group`] / [`check_partitions`], so a product
//! the seal accepts is one every reader resolves the same way.
//!
//! A table of two or more blocks is written as the leaves `<table>/0000`, `<table>/0001`, …, each
//! carrying a [`Partition`] marker in its sealed [`TableSpec`]. The leaf of index `i` is
//! `format!("{i:04}")`: padded to four digits, never longer than needed (`10000` past `9999`), and
//! the manifest order is the index order. Products written before ADR-0064 name their blocks
//! `<table>_NNNN` without a marker; that **legacy flat** form resolves as it always has.
#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

use super::table::TableSpec;
use super::{BlockKind, BlockName, BlockRef};
use crate::{Error, Result};

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

/// The canonical name of leaf `index` of the partitioned table `table`: `<table>/{index:04}`, checked
/// against the block-name grammar. A `table` the grammar refuses (`""` would give `/0000`), or one
/// over 234 bytes that pushes the leaf past the 255-byte limit (ADR-0064 §1.3), is
/// [`Error::InvalidBlockName`].
pub fn leaf_name(table: &str, index: u64) -> Result<BlockName> {
    BlockName::new(format!("{table}/{}", leaf_segment(index)))
}

/// One class of block a partitioned node may hold besides its leaves (ADR-0064 §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildClass {
    /// The class's name, for diagnostics.
    pub class: &'static str,
    /// The suffix appended to an existing leaf's full name (`<table>/<leaf><suffix>`).
    pub leaf_suffix: &'static str,
    /// The block kind the child must have.
    pub kind: BlockKind,
    /// The `spec` key that must name the leaf (its full name) the child belongs to.
    pub leaf_key: &'static str,
}

/// The closed list of child classes allowed under a partitioned node (ADR-0064 §2.1). Anything
/// else under the node is a foreign child, which refuses the logical name: adding a class is an
/// amendment of ADR-0064.
pub const PARTITION_CHILD_CLASSES: &[ChildClass] = &[ChildClass {
    class: "chunk-index sidecar",
    leaf_suffix: ".cidx",
    kind: BlockKind::ChunkIndex,
    leaf_key: "indexes",
}];

/// What a table name resolves to (ADR-0064 §3), over the manifest alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableGroup {
    /// `name` is one `table` block and nothing else answers to it.
    Single(BlockName),
    /// `name` is a valid partitioned node; its leaves in index order, which is their manifest order.
    Partitioned {
        /// The leaves, `<name>/0000` first.
        leaves: Vec<BlockName>,
    },
    /// Blocks of the legacy `<name>_NNNN` form (and the bare `name`), in manifest order.
    LegacyFlat {
        /// The matching blocks, in manifest order.
        blocks: Vec<BlockName>,
    },
}

impl TableGroup {
    /// The group's blocks in logical order: the one block, the leaves in index order, or the
    /// legacy blocks in manifest order.
    pub fn blocks(&self) -> &[BlockName] {
        match self {
            TableGroup::Single(b) => std::slice::from_ref(b),
            TableGroup::Partitioned { leaves } => leaves,
            TableGroup::LegacyFlat { blocks } => blocks,
        }
    }
}

/// Why a table name does not resolve to a logical table (ADR-0064 §3), carried by
/// [`Error::LogicalTable`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LogicalTableError {
    /// No block and no node has the name.
    #[error("no block or table of that name")]
    NotFound,
    /// The name is a block of another kind, or a node (`kind: "node"`) with no marked leaf.
    #[error("it is a {kind}, not a table")]
    NotATable {
        /// The block's kind (`array`, `blob`, …) or `node`.
        kind: String,
    },
    /// Both the legacy flat form and a partitioned node answer to the name.
    #[error(
        "ambiguous: legacy blocks {flat:?} and the partitioned node '{node}/' both answer to it"
    )]
    Ambiguous {
        /// The legacy flat blocks, in manifest order.
        flat: Vec<String>,
        /// The partitioned node.
        node: String,
    },
    /// A block with no parent node carries a partition marker.
    #[error("block '{block}' carries a partition marker but is not a leaf of a node")]
    StrayMarker {
        /// The marked block.
        block: String,
    },
    /// A partitioned node with one leaf: a table of one block is the bare name.
    #[error("partitioned table has the single leaf '{leaf}' (a one-block table is the bare name)")]
    SingleLeaf {
        /// The only leaf.
        leaf: String,
    },
    /// A block under the node that is neither a leaf nor of a registered child class.
    #[error("'{child}' is neither a leaf of the table nor a registered child")]
    ForeignChild {
        /// The foreign block.
        child: String,
    },
    /// A leaf whose segment is not the canonical `{index:04}` of its marker.
    #[error("leaf '{leaf}' should be named '{expected}' for its marker's index")]
    NonCanonicalLeaf {
        /// The leaf as named.
        leaf: String,
        /// The canonical name of its index.
        expected: String,
    },
    /// Two leaves carry the same index.
    #[error("index {index} appears on two leaves")]
    DuplicateIndex {
        /// The repeated index.
        index: u64,
    },
    /// An index below the highest one has no leaf.
    #[error("no leaf for index {missing}")]
    Gap {
        /// The lowest missing index.
        missing: u64,
    },
    /// No leaf is marked `last`: the tail of the table is missing.
    #[error("no leaf is marked last (highest index {max_index})")]
    NoTerminal {
        /// The highest index present.
        max_index: u64,
    },
    /// A leaf comes after the one marked `last`.
    #[error("leaf '{leaf}' comes after the leaf marked last")]
    AfterTerminal {
        /// The leaf past the terminal.
        leaf: String,
    },
    /// Leaves out of increasing index order in the manifest.
    #[error("leaf '{leaf}' is listed after '{after}', which has a higher index")]
    OutOfOrder {
        /// The leaf listed too late.
        leaf: String,
        /// The leaf listed before it.
        after: String,
    },
    /// A leaf whose columns differ from the first leaf's in count, name or dtype.
    #[error("leaf '{leaf}' does not match the first leaf's schema: {detail}")]
    SchemaMismatch {
        /// The mismatching leaf.
        leaf: String,
        /// What differs.
        detail: String,
    },
}

/// Why a streaming table writer refused its input or its call (ADR-0064 §5.3, §5.4), carried by
/// [`Error::StreamedTable`]. Nothing of the table is sealed when it is raised.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StreamedTableError {
    /// The input yielded a row past the declared `spec.rows`.
    #[error("the input runs past the declared {declared} rows")]
    TooManyRows {
        /// The declared row count.
        declared: u64,
    },
    /// The input ended before the declared `spec.rows`.
    #[error("the input ended after {actual} of the declared {declared} rows")]
    TooFewRows {
        /// The declared row count.
        declared: u64,
        /// The rows the input yielded.
        actual: u64,
    },
    /// One row-group of the input holds more rows than a row-group may.
    #[error("a row-group of {rows} rows exceeds the {max} rows of one row-group")]
    OversizedGroup {
        /// The group's rows.
        rows: u64,
        /// The most rows a group may hold.
        max: u64,
    },
    /// A single-block writer was given more rows than one block holds.
    #[error(
        "{rows} rows exceed one block of {max} rows: write the table with StreamWriter::push_table"
    )]
    NeedsPartition {
        /// The declared row count.
        rows: u64,
        /// The most rows of one block.
        max: u64,
    },
}

/// The legacy flat rule (ADR-0064 §4, `Reader::block_group` before ADR-0064): `prefix` itself, or
/// `prefix_` followed by exactly four ASCII digits.
pub fn is_legacy_flat_member(name: &str, prefix: &str) -> bool {
    name == prefix
        || name
            .strip_prefix(prefix)
            .and_then(|r| r.strip_prefix('_'))
            .is_some_and(|r| r.len() == 4 && r.bytes().all(|c| c.is_ascii_digit()))
}

/// The partition marker a block's spec carries, if any. A `partition` key that is not a
/// [`Partition`] is refused rather than read as unmarked.
pub fn marker(block: &BlockRef) -> Result<Option<Partition>> {
    match block.spec.get("partition") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| {
            Error::Invalid(format!(
                "block '{}': malformed partition marker: {e}",
                block.name
            ))
        }),
    }
}

/// Resolve the table name `name` over a manifest's `blocks` (ADR-0064 §3), reading no payload.
pub fn table_group(blocks: &[BlockRef], name: &str) -> Result<TableGroup> {
    let fail = |reason| Error::LogicalTable {
        name: name.to_string(),
        reason,
    };
    let mut marked = Vec::new();
    for b in blocks.iter().filter(|b| is_child_of(&b.name, name)) {
        if marker(b)?.is_some() {
            marked.push(b);
        }
    }
    let flat: Vec<&BlockRef> = blocks
        .iter()
        .filter(|b| is_legacy_flat_member(&b.name, name))
        .collect();
    if !marked.is_empty() && !flat.is_empty() {
        let flat = flat.iter().map(|b| b.name.to_string()).collect();
        return Err(fail(LogicalTableError::Ambiguous {
            flat,
            node: name.to_string(),
        }));
    }
    if !marked.is_empty() {
        return validate_node(blocks, name).map(|leaves| TableGroup::Partitioned { leaves });
    }
    // A marked block in the legacy group is checked as the seal checks it, so a reader refuses
    // what the seal refuses (§3 rule 4, #733). The one exception is a leaf named in full
    // (`pairs/0003`) that is the whole group, which stays readable on its own. Next to a
    // `pairs/0003_0000` it is not exempt: that block is a foreign child of `pairs`, not more rows.
    for b in &flat {
        let leaf_in_full = flat.len() == 1 && b.name == name && b.name.contains('/');
        if !leaf_in_full && marker(b)?.is_some() {
            check_marked(blocks, b)?;
        }
    }
    match flat.as_slice() {
        [b] if b.name == name && b.kind == BlockKind::Table => {
            Ok(TableGroup::Single(b.name.clone()))
        }
        [b] if b.name == name => Err(fail(LogicalTableError::NotATable {
            kind: kind_name(b.kind),
        })),
        [] if blocks.iter().any(|b| {
            b.name
                .strip_prefix(name)
                .is_some_and(|r| r.starts_with('/'))
        }) =>
        {
            Err(fail(LogicalTableError::NotATable {
                kind: "node".into(),
            }))
        }
        [] => Err(fail(LogicalTableError::NotFound)),
        _ => Ok(TableGroup::LegacyFlat {
            blocks: flat.iter().map(|b| b.name.clone()).collect(),
        }),
    }
}

/// The seal-time check (ADR-0064 §2.4): every node holding a marked leaf is a valid partitioned
/// table that no legacy flat block also answers to, and no block without a parent node is marked.
/// Run wherever a set of names is finalised, before anything is written.
pub fn check_partitions(blocks: &[BlockRef]) -> Result<()> {
    let mut nodes: Vec<&str> = Vec::new();
    for b in blocks {
        if marker(b)?.is_none() {
            continue;
        }
        let node = b.name.rsplit_once('/').map(|(node, _)| node);
        if node.is_none_or(|n| !nodes.contains(&n)) {
            check_marked(blocks, b)?;
            nodes.extend(node);
        }
    }
    Ok(())
}

/// The seal's rule for one marked block `b` (ADR-0064 §2.2, §2.4): with no parent node it is a
/// stray marker; otherwise its parent node must be a valid partitioned table.
fn check_marked(blocks: &[BlockRef], b: &BlockRef) -> Result<()> {
    match b.name.rsplit_once('/') {
        None => Err(Error::LogicalTable {
            name: b.name.to_string(),
            reason: LogicalTableError::StrayMarker {
                block: b.name.to_string(),
            },
        }),
        Some((node, _)) => table_group(blocks, node).map(drop),
    }
}

/// Validate the partitioned node `node` (ADR-0064 §2.1, §2.5); its leaves in index order.
fn validate_node(blocks: &[BlockRef], node: &str) -> Result<Vec<BlockName>> {
    let fail = |reason| Error::LogicalTable {
        name: node.to_string(),
        reason,
    };
    // Every block under `node/`, at any depth, in manifest order, with its marker if it is one
    // segment below (only those can be leaves).
    let mut under: Vec<(&BlockRef, Option<Partition>)> = Vec::new();
    for b in blocks.iter().filter(|b| {
        b.name
            .strip_prefix(node)
            .is_some_and(|r| r.starts_with('/'))
    }) {
        let p = if is_child_of(&b.name, node) {
            marker(b)?
        } else {
            None
        };
        under.push((b, p));
    }
    let leaf_names: std::collections::HashSet<&str> = under
        .iter()
        .filter(|(_, p)| p.is_some())
        .map(|(b, _)| b.name.as_str())
        .collect();
    let is_leaf = |n: &str| leaf_names.contains(n);
    let mut leaves: Vec<(&BlockRef, Partition)> = Vec::with_capacity(leaf_names.len());
    let mut prev: Option<(&BlockRef, Partition)> = None;
    for &(b, p) in &under {
        let Some(p) = p else {
            if !PARTITION_CHILD_CLASSES
                .iter()
                .any(|c| is_member(c, b, &is_leaf))
            {
                return Err(fail(LogicalTableError::ForeignChild {
                    child: b.name.to_string(),
                }));
            }
            continue;
        };
        // The canonical name as a string, not through `leaf_name`: a forged index can make it too
        // long for the grammar, and that leaf is just as non-canonical as `pairs/00003` (#733).
        let expected = format!("{node}/{}", leaf_segment(p.index));
        if b.kind != BlockKind::Table {
            return Err(fail(LogicalTableError::ForeignChild {
                child: b.name.to_string(),
            }));
        }
        if b.name != expected {
            return Err(fail(LogicalTableError::NonCanonicalLeaf {
                leaf: b.name.to_string(),
                expected,
            }));
        }
        if let Some((pb, pp)) = prev {
            if pp.index == p.index {
                return Err(fail(LogicalTableError::DuplicateIndex { index: p.index }));
            }
            if pp.index > p.index {
                let (leaf, after) = (b.name.to_string(), pb.name.to_string());
                return Err(fail(LogicalTableError::OutOfOrder { leaf, after }));
            }
            if pp.last {
                return Err(fail(LogicalTableError::AfterTerminal {
                    leaf: b.name.to_string(),
                }));
            }
        }
        prev = Some((b, p));
        leaves.push((b, p));
    }
    if let Some(missing) = (0u64..)
        .zip(&leaves)
        .find(|(i, (_, p))| p.index != *i)
        .map(|(i, _)| i)
    {
        return Err(fail(LogicalTableError::Gap { missing }));
    }
    let Some(&(last, lp)) = leaves.last() else {
        let kind = "node".into();
        return Err(fail(LogicalTableError::NotATable { kind }));
    };
    if leaves.len() == 1 {
        return Err(fail(LogicalTableError::SingleLeaf {
            leaf: last.name.to_string(),
        }));
    }
    if !lp.last {
        return Err(fail(LogicalTableError::NoTerminal {
            max_index: lp.index,
        }));
    }
    let refs: Vec<&BlockRef> = leaves.iter().map(|(b, _)| *b).collect();
    specs_of(node, &refs)?;
    Ok(leaves.into_iter().map(|(b, _)| b.name.clone()).collect())
}

/// The [`TableSpec`] of every block of a resolved group, in the group's order (ADR-0064 §2.1's
/// schema rule): each block's spec must parse, and every block must have the first one's column
/// names and dtypes, in order, or the name is refused with [`LogicalTableError::SchemaMismatch`].
/// The validation of a partitioned node already applies it; readers apply it to every group, the
/// legacy flat form included, through this one function. Reads the manifest only.
///
/// `group` must come from [`table_group`] over the same `blocks`. A group naming a block that
/// `blocks` lacks (resolved over another manifest) is [`LogicalTableError::NotFound`].
pub fn group_specs(blocks: &[BlockRef], name: &str, group: &TableGroup) -> Result<Vec<TableSpec>> {
    let by_name: std::collections::HashMap<&str, &BlockRef> =
        blocks.iter().map(|b| (b.name.as_str(), b)).collect();
    let refs = group
        .blocks()
        .iter()
        .map(|n| {
            // Unreachable for a `group` that `table_group` resolved over these same `blocks`: every
            // name it returns is one of them. Only a group from another manifest misses, and that
            // is the caller's error, reported as a typed `NotFound` rather than a panic.
            by_name
                .get(n.as_str())
                .copied()
                .ok_or_else(|| Error::LogicalTable {
                    name: name.to_string(),
                    reason: LogicalTableError::NotFound,
                })
        })
        .collect::<Result<Vec<_>>>()?;
    specs_of(name, &refs)
}

/// Parse each block's [`TableSpec`] and check the shared schema (see [`group_specs`]).
fn specs_of(name: &str, refs: &[&BlockRef]) -> Result<Vec<TableSpec>> {
    let mismatch = |b: &BlockRef, detail: String| Error::LogicalTable {
        name: name.to_string(),
        reason: LogicalTableError::SchemaMismatch {
            leaf: b.name.to_string(),
            detail,
        },
    };
    let columns = |s: &TableSpec| {
        s.columns
            .iter()
            .map(|c| (c.name.clone(), c.dtype.clone()))
            .collect::<Vec<_>>()
    };
    let mut specs: Vec<TableSpec> = Vec::with_capacity(refs.len());
    for b in refs {
        let spec: TableSpec =
            serde_json::from_value(b.spec.clone()).map_err(|e| mismatch(b, e.to_string()))?;
        if let Some(head) = specs.first().map(columns) {
            let cols = columns(&spec);
            if cols != head {
                return Err(mismatch(
                    b,
                    format!("columns {cols:?}, first leaf {head:?}"),
                ));
            }
        }
        specs.push(spec);
    }
    Ok(specs)
}

/// Whether `name` is exactly one segment below `node` (`node/<segment>`, no further `/`).
fn is_child_of(name: &str, node: &str) -> bool {
    name.strip_prefix(node)
        .and_then(|r| r.strip_prefix('/'))
        .is_some_and(|seg| !seg.contains('/'))
}

/// Whether `b` is a child of class `c` of an existing leaf (`is_leaf`).
fn is_member(c: &ChildClass, b: &BlockRef, is_leaf: &dyn Fn(&str) -> bool) -> bool {
    b.kind == c.kind
        && b.name.strip_suffix(c.leaf_suffix).is_some_and(|leaf| {
            is_leaf(leaf) && b.spec.get(c.leaf_key).and_then(|v| v.as_str()) == Some(leaf)
        })
}

/// A block kind's manifest spelling (`array`, `chunk_index`, …).
fn kind_name(kind: BlockKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::name::{NameError, MAX_BLOCK_NAME_BYTES};
    use crate::block::table::Column;
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
            .map(|i| table(&leaf_name("pairs", i).unwrap(), Some((i, i + 1 == n))))
            .collect()
    }

    fn reason(blocks: &[BlockRef], name: &str) -> LogicalTableError {
        match table_group(blocks, name) {
            Err(Error::LogicalTable { reason, .. }) => reason,
            other => panic!("expected a LogicalTable error, got {other:?}"),
        }
    }

    fn names(v: &[&str]) -> Vec<BlockName> {
        v.iter().map(|n| n.parse().unwrap()).collect()
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
        let name = |t: &str, i| leaf_name(t, i).unwrap();
        assert_eq!(name("pairs", 3), "pairs/0003");
        assert_eq!(name("pairs", 9999), "pairs/9999");
        assert_eq!(name("pairs", 10000), "pairs/10000");
        assert_eq!(leaf_segment(u64::MAX), u64::MAX.to_string());
        // §1.3: a 234-byte table takes every u64 index; one byte more is refused by the grammar.
        let t234 = "t".repeat(234);
        assert_eq!(name(&t234, u64::MAX).len(), MAX_BLOCK_NAME_BYTES);
        let t235 = "t".repeat(235);
        for (table, index, want) in [
            ("", 0, NameError::EmptySegment),
            ("a/", 0, NameError::EmptySegment),
            ("a b", 0, NameError::DisallowedChar(' ')),
            (t235.as_str(), u64::MAX, NameError::TooLong { len: 256 }),
        ] {
            match leaf_name(table, index) {
                Err(Error::InvalidBlockName { reason, .. }) => {
                    assert_eq!(reason, want, "{table:?}")
                }
                other => panic!("leaf_name({table:?}, {index}) must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn index_10000_resolves_after_9999_in_manifest_order() {
        let all = leaves(10_001);
        let Ok(TableGroup::Partitioned { leaves }) = table_group(&all, "pairs") else {
            panic!("a 10001-leaf partition is valid");
        };
        assert_eq!(leaves[9999], "pairs/9999");
        assert_eq!(leaves[10000], "pairs/10000");
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
    /// The digests are synthetic (`hash::digest` of the name), so this pins the manifest side only;
    /// comparing a partitioned write's payload digests with a single-block write of the same rows is
    /// the writer's test, ADR-0064 PR D (#712).
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
            b.name = leaf_name("rows", i as u64).unwrap();
        }
        for other in [seal(unmarked).unwrap(), seal(renamed).unwrap()] {
            assert_ne!(other.manifest_hash, base.manifest_hash);
            assert_eq!(digests_of(&other), digests_of(&base));
        }
        // Deterministic: the same partition seals the same manifest.
        assert_eq!(seal(leaves(2)).unwrap().manifest_hash, base.manifest_hash);
    }

    #[test]
    fn a_valid_partition_resolves_with_its_cidx_sidecars() {
        let mut blocks = leaves(3);
        let cidx = |leaf: &str| {
            let spec = json!({"class": "derived", "indexes": leaf});
            block(&format!("{leaf}.cidx"), BlockKind::ChunkIndex, spec)
        };
        blocks.insert(1, cidx("pairs/0000"));
        blocks.push(cidx("pairs/0002"));
        let want = names(&["pairs/0000", "pairs/0001", "pairs/0002"]);
        assert_eq!(
            table_group(&blocks, "pairs").unwrap(),
            TableGroup::Partitioned { leaves: want }
        );
        seal(blocks.clone()).unwrap();
        // A sidecar of no existing leaf, or one naming another leaf, is foreign.
        for bad in [cidx("pairs/0007"), {
            let mut c = cidx("pairs/0001");
            c.name = "pairs/0000.cidx".parse().unwrap();
            c
        }] {
            let mut b = leaves(3);
            b.push(bad.clone());
            let child = bad.name.to_string();
            assert_eq!(
                reason(&b, "pairs"),
                LogicalTableError::ForeignChild { child }
            );
        }
    }

    /// Every validity rule of ADR-0064 §2.1/§2.5 and its typed error, at read and at seal.
    #[test]
    fn each_invalid_node_gives_its_variant_at_read_and_at_seal() {
        use LogicalTableError as E;
        let s = |v: &str| v.to_string();
        let with = |mut b: Vec<BlockRef>, extra: BlockRef| {
            b.push(extra);
            b
        };
        let mut swapped = leaves(2);
        swapped.swap(0, 1);
        let mut dup = leaves(3);
        dup[1] = table("pairs/0000", Some((0, false)));
        let mut wide = leaves(2);
        wide[1].spec = spec(
            &[("t", "u8"), ("e", "i2")],
            Some(Partition {
                index: 1,
                last: true,
            }),
        );
        let cases: Vec<(Vec<BlockRef>, E)> = vec![
            (
                with(leaves(3), table("pairs/0003", None)),
                E::ForeignChild {
                    child: s("pairs/0003"),
                },
            ),
            (
                with(leaves(3), block("pairs/view", BlockKind::Array, json!({}))),
                E::ForeignChild {
                    child: s("pairs/view"),
                },
            ),
            (
                with(leaves(2), table("pairs/sub/x", None)),
                E::ForeignChild {
                    child: s("pairs/sub/x"),
                },
            ),
            (
                vec![
                    table("pairs/0000", Some((0, false))),
                    block(
                        "pairs/0001",
                        BlockKind::Array,
                        json!({"partition": {"index": 1, "last": true}}),
                    ),
                ],
                E::ForeignChild {
                    child: s("pairs/0001"),
                },
            ),
            (
                vec![
                    table("pairs/0000", Some((0, false))),
                    table("pairs/00001", Some((1, true))),
                ],
                E::NonCanonicalLeaf {
                    leaf: s("pairs/00001"),
                    expected: s("pairs/0001"),
                },
            ),
            (
                vec![
                    table("pairs/0000", Some((0, false))),
                    table("pairs/1", Some((1, true))),
                ],
                E::NonCanonicalLeaf {
                    leaf: s("pairs/1"),
                    expected: s("pairs/0001"),
                },
            ),
            (
                vec![
                    table("pairs/0000", Some((0, false))),
                    table("pairs/abcd", Some((1, true))),
                ],
                E::NonCanonicalLeaf {
                    leaf: s("pairs/abcd"),
                    expected: s("pairs/0001"),
                },
            ),
            (
                with(
                    leaves(2),
                    block(
                        "pairs/0000.cidx",
                        BlockKind::ChunkIndex,
                        json!({"indexes": "pairs/0000", "partition": {"index": 2, "last": true}}),
                    ),
                ),
                E::ForeignChild {
                    child: s("pairs/0000.cidx"),
                },
            ),
            (dup, E::DuplicateIndex { index: 0 }),
            (
                vec![
                    table("pairs/0000", Some((0, false))),
                    table("pairs/0002", Some((2, true))),
                ],
                E::Gap { missing: 1 },
            ),
            (
                vec![
                    table("pairs/0001", Some((1, false))),
                    table("pairs/0002", Some((2, true))),
                ],
                E::Gap { missing: 0 },
            ),
            (
                vec![
                    table("pairs/0000", Some((0, false))),
                    table("pairs/0001", Some((1, false))),
                ],
                E::NoTerminal { max_index: 1 },
            ),
            (
                vec![
                    table("pairs/0000", Some((0, true))),
                    table("pairs/0001", Some((1, true))),
                ],
                E::AfterTerminal {
                    leaf: s("pairs/0001"),
                },
            ),
            (
                with(leaves(2), table("pairs/0002", Some((2, true)))),
                E::AfterTerminal {
                    leaf: s("pairs/0002"),
                },
            ),
            (
                swapped,
                E::OutOfOrder {
                    leaf: s("pairs/0000"),
                    after: s("pairs/0001"),
                },
            ),
            (
                vec![table("pairs/0000", Some((0, true)))],
                E::SingleLeaf {
                    leaf: s("pairs/0000"),
                },
            ),
            (
                with(leaves(2), table("pairs_0000", None)),
                E::Ambiguous {
                    flat: vec![s("pairs_0000")],
                    node: s("pairs"),
                },
            ),
        ];
        for (blocks, want) in cases {
            assert_eq!(reason(&blocks, "pairs"), want, "read: {want:?}");
            match seal(blocks) {
                Err(Error::LogicalTable { name, reason }) if name == "pairs" => {
                    assert_eq!(reason, want, "seal")
                }
                other => panic!("seal of {want:?} must be refused, got {other:?}"),
            }
        }
        assert!(
            matches!(reason(&wide, "pairs"), E::SchemaMismatch { leaf, .. } if leaf == "pairs/0001")
        );
        assert!(matches!(
            seal(wide),
            Err(Error::LogicalTable {
                reason: E::SchemaMismatch { .. },
                ..
            })
        ));
    }

    /// #733: a forged index whose canonical name would break the 255-byte limit is a leaf not
    /// named for its index, so `NonCanonicalLeaf`, not a bare block-name error.
    #[test]
    fn a_forged_index_past_the_name_limit_is_non_canonical() {
        let node = "t".repeat(240);
        let leaf = format!("{node}/0001");
        let blocks = vec![
            table(&format!("{node}/0000"), Some((0, false))),
            table(&leaf, Some((u64::MAX, true))),
        ];
        let expected = format!("{node}/{}", u64::MAX);
        let want = LogicalTableError::NonCanonicalLeaf { leaf, expected };
        assert_eq!(reason(&blocks, &node), want);
        assert!(matches!(seal(blocks), Err(Error::LogicalTable { reason, .. }) if reason == want));
    }

    #[test]
    fn a_marker_without_a_parent_node_is_stray_at_read_and_at_seal() {
        for (blocks, name, block) in [
            (vec![table("pairs", Some((0, true)))], "pairs", "pairs"),
            (
                vec![
                    table("pairs_0000", None),
                    table("pairs_0001", Some((1, true))),
                ],
                "pairs",
                "pairs_0001",
            ),
        ] {
            let want = LogicalTableError::StrayMarker {
                block: block.into(),
            };
            // Read and seal report the marked block itself as the name (#733: one rule).
            for got in [table_group(&blocks, name).map(drop), seal(blocks).map(drop)] {
                assert!(
                    matches!(&got, Err(Error::LogicalTable { name: n, reason }) if n == block && *reason == want),
                    "{got:?}"
                );
            }
        }
    }

    /// #733: a marked legacy-named block under a node (`a/pairs_0001`) is a leaf of `a` that isn't
    /// canonical. Reading `a/pairs` refuses it with the same error as the seal, from `a`'s
    /// validation, instead of resolving a legacy flat group around it.
    #[test]
    fn a_marked_nested_legacy_block_is_refused_at_read_as_at_seal() {
        let blocks = vec![
            table("a/pairs_0000", None),
            table("a/pairs_0001", Some((1, true))),
        ];
        let want = LogicalTableError::ForeignChild {
            child: "a/pairs_0000".into(),
        };
        for got in [
            table_group(&blocks, "a/pairs").map(drop),
            seal(blocks).map(drop),
        ] {
            match got {
                Err(Error::LogicalTable { name, reason }) => {
                    assert_eq!((name.as_str(), &reason), ("a", &want))
                }
                other => panic!("expected node 'a' to be refused, got {other:?}"),
            }
        }
        // A marked leaf named in full is exempt only when it is the whole group: an unmarked
        // `pairs/0001_0000` next to it (an old binary's `commit --add-block`) would otherwise be
        // read as its legacy continuation, while the seal refuses the node `pairs`.
        let mut blocks = leaves(2);
        blocks.push(table("pairs/0001_0000", None));
        let want = LogicalTableError::ForeignChild {
            child: "pairs/0001_0000".into(),
        };
        for got in [
            table_group(&blocks, "pairs/0001").map(drop),
            seal(blocks).map(drop),
        ] {
            match got {
                Err(Error::LogicalTable { name, reason }) => {
                    assert_eq!((name.as_str(), &reason), ("pairs", &want))
                }
                other => panic!("expected node 'pairs' to be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn single_names_and_non_tables() {
        use LogicalTableError as E;
        let mut blocks = leaves(4);
        blocks.push(table("small", None));
        blocks.push(block("volume/0", BlockKind::Array, json!({})));
        blocks.push(block("ct", BlockKind::Array, json!({})));
        // A marked leaf named in full, and a bare small table, are single blocks.
        assert_eq!(
            table_group(&blocks, "pairs/0003").unwrap(),
            TableGroup::Single(names(&["pairs/0003"])[0].clone())
        );
        assert_eq!(
            table_group(&blocks, "small").unwrap(),
            TableGroup::Single(names(&["small"])[0].clone())
        );
        assert_eq!(
            reason(&blocks, "ct"),
            E::NotATable {
                kind: "array".into()
            }
        );
        assert_eq!(
            reason(&blocks, "volume"),
            E::NotATable {
                kind: "node".into()
            }
        );
        assert_eq!(reason(&blocks, "missing"), E::NotFound);
        assert_eq!(reason(&blocks, "pair"), E::NotFound);
    }

    /// ADR-0064 §4: the alpha.1–4 `events_NNNN` shape resolves exactly as `Reader::block_group` did
    /// (the cases of `tessera-io/src/container.rs`), and an unmarked `events/notes` takes no part.
    #[test]
    fn legacy_flat_products_resolve_as_before() {
        let blocks: Vec<BlockRef> = [
            "events_0001",
            "events_0000",
            "noise",
            "events",
            "events_index",
            "events_99",
            "events_0010",
        ]
        .iter()
        .map(|n| table(n, None))
        .collect();
        let want = names(&["events_0001", "events_0000", "events", "events_0010"]);
        assert_eq!(
            table_group(&blocks, "events").unwrap(),
            TableGroup::LegacyFlat { blocks: want }
        );

        let mut alpha = vec![
            table("events_0000", None),
            table("events_0001", None),
            table("events_0002", None),
        ];
        alpha.push(block(
            "events/notes",
            BlockKind::Blob,
            json!({"filename": "n", "size": 1}),
        ));
        let want = names(&["events_0000", "events_0001", "events_0002"]);
        assert_eq!(
            table_group(&alpha, "events").unwrap(),
            TableGroup::LegacyFlat { blocks: want }
        );
        seal(alpha).unwrap();
    }

    /// `group_specs` applies the schema rule to a legacy flat group too: a block whose spec isn't a
    /// `TableSpec` is `SchemaMismatch`, named by the block, not a bare `Error::Serde`.
    #[test]
    fn a_legacy_block_with_an_unparsable_spec_is_a_schema_mismatch() {
        let mut blocks = vec![table("events_0000", None), table("events_0001", None)];
        blocks[1].spec = json!({"rows": "many"});
        let group = table_group(&blocks, "events").unwrap();
        match group_specs(&blocks, "events", &group) {
            Err(Error::LogicalTable {
                name,
                reason: LogicalTableError::SchemaMismatch { leaf, .. },
            }) => assert_eq!((name.as_str(), leaf.as_str()), ("events", "events_0001")),
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_malformed_marker_is_refused_not_ignored() {
        let mut blocks = leaves(2);
        blocks[1].spec["partition"] = json!(1);
        assert!(matches!(
            table_group(&blocks, "pairs"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(seal(blocks), Err(Error::Invalid(_))));
    }
}
