//! Storage blocks — the shape-dispatched payloads of a product.
//!
//! A product holds N blocks. Each block is one *shape*, chosen by the data:
//! - [`array`] — dense N-D chunked arrays (volumes, sinograms) → zarrs backend.
//! - [`table`] — columnar tables (events, spectra, ROIs) → arrow/parquet backend.
//! - [`blob`] — opaque bytes preserved verbatim (un-parsed vendor files) → no codec, `blake3` only.
//!
//! The core records each block as a [`BlockRef`] in the manifest and rolls a primary block's digest
//! into the product Merkle root ([`content_root`]); a derived block ([`BlockClass`]) is covered by the
//! seal only. Payload read/write lives behind the backend features.

pub mod array;
pub mod blob;
pub mod name;
pub mod partition;
pub mod table;

pub use name::{check_leaf_rule, check_unique_names, BlockName};
pub use partition::{
    check_partitions, group_specs, table_group, LogicalTableError, Partition, StreamedTableError,
    TableGroup,
};

pub use table::{ContractField, ContractValue, SchemaMismatchKind};

use serde::{Deserialize, Serialize};

/// Which engine/shape a block uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Dense N-D chunked array (zarrs). Volumes, sinograms, μ-maps.
    Array,
    /// Columnar table (arrow/parquet). Events, spectra, ROIs.
    Table,
    /// A `{hash, stats}` chunk-index over another block's sub-blocks (ADR-0028 §3) — an **additive**
    /// companion block (per-chunk confirmation + pruning) that floats on top of the data block it
    /// indexes without changing that block's digest.
    ChunkIndex,
    /// Opaque bytes preserved **verbatim** — the "junk"/preservation tier (ADR-0038). A vendor file the
    /// engine does not parse (Siemens `.l64` listmode, GE `.7z`, a PDF): stored bit-faithfully, no codec,
    /// digest = `blake3(bytes)`. See [`blob`].
    Blob,
}

/// Whether a block is part of the product's identity (ADR-0028 §4, SPEC §4/§5).
///
/// A **primary** block is source data: its digest is a leaf of `content_hash`. A **derived** block is
/// an accelerator, a pure function of primary data that a reader may drop and rebuild (the chunk
/// index `.cidx`, the levels of a multiscale pyramid above level 0). It is not part of
/// `content_hash`, so adding, regenerating or dropping one leaves the data fingerprint unchanged;
/// `manifest_hash` still covers it, its digest and its class, because the block ref is in the stored
/// manifest.
///
/// In the manifest the class is the block ref's `class` key: absent for a primary block (the key is
/// never written for one, so a product without derived blocks keeps its bytes) and `"derived"` for a
/// derived one. Any other value, `"primary"` and `null` included, is refused on reading, so a class
/// has exactly one spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlockClass {
    /// Source data: a leaf of `content_hash`. Written as no `class` key.
    #[default]
    Primary,
    /// A regenerable accelerator: covered by `manifest_hash`, not by `content_hash`. Written as
    /// `"class": "derived"`.
    Derived,
}

impl BlockClass {
    /// The one spelling of [`BlockClass::Derived`] in a manifest.
    pub const DERIVED: &'static str = "derived";

    /// True for [`BlockClass::Primary`]: the block's digest is a leaf of `content_hash`.
    pub fn is_primary(&self) -> bool {
        matches!(self, BlockClass::Primary)
    }
}

impl Serialize for BlockClass {
    /// Only [`BlockClass::Derived`] has a spelling. A [`BlockRef`] never writes the key for a primary
    /// block, so serialising [`BlockClass::Primary`] on its own is refused rather than given a second
    /// spelling no reader accepts.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            BlockClass::Derived => s.serialize_str(Self::DERIVED),
            BlockClass::Primary => Err(serde::ser::Error::custom(
                "a primary block has no `class` spelling: the key is omitted",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for BlockClass {
    /// Accepts exactly `"derived"`. A present `class` key with any other value (`"primary"`, `null`,
    /// another string or type) is refused: a primary block is written without the key.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v.as_str() {
            Some(Self::DERIVED) => Ok(BlockClass::Derived),
            _ => Err(serde::de::Error::custom(format!(
                "block class {v} is not \"{}\": a primary block carries no `class` key",
                Self::DERIVED
            ))),
        }
    }
}

/// A reference to a block, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRef {
    /// The block's name: validated by the block-name grammar (ADR-0063), on construction and on
    /// deserialization alike.
    pub name: BlockName,
    pub kind: BlockKind,
    /// Per-block content digest. A primary block's is a leaf of the product's `content_hash`; every
    /// block's is covered by `manifest_hash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Whether the block is part of `content_hash` ([`BlockClass`]). Omitted from the manifest when
    /// primary.
    #[serde(default, skip_serializing_if = "BlockClass::is_primary")]
    pub class: BlockClass,
    /// Shape-specific descriptor (dtype, chunks, schema, codec, ...) for self-description.
    #[serde(default)]
    pub spec: serde_json::Value,
}

impl BlockRef {
    /// The JSON keys this type models (ADR-0060 §2.2 `newer_fields`); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] = &["name", "kind", "digest", "class", "spec"];

    /// This reference marked [`BlockClass::Derived`]: the writer of an accelerator block (a chunk
    /// index, a pyramid level) sets it, so the block leaves `content_hash` (ADR-0028 §4).
    pub fn derived(mut self) -> Self {
        self.class = BlockClass::Derived;
        self
    }

    /// True when the block's digest is a leaf of `content_hash` ([`BlockClass::Primary`]).
    pub fn is_primary(&self) -> bool {
        self.class.is_primary()
    }

    /// True when the block is a chunk index by what it is, whatever it is called and whatever its
    /// class: its kind is [`BlockKind::ChunkIndex`], or its spec names a block it indexes
    /// (`indexes`, SPEC §5d). The test `tessera commit --add-block` refuses a copy by (#720): a chunk
    /// index is computed from the data by the writer, never copied, so a `.cidx` suffix is neither
    /// needed nor enough to be one.
    pub fn is_chunk_index(&self) -> bool {
        self.kind == BlockKind::ChunkIndex || self.spec.get("indexes").is_some()
    }
}

/// Enforce that every **chunk index** of one product ([`BlockRef::is_chunk_index`]) indexes another
/// block of the same product: its spec's `indexes` is a string naming a listed block other than the
/// index itself. The first index that doesn't is [`crate::Error::DanglingChunkIndex`], naming it and
/// the block it names. Readers apply it too, like [`check_unique_names`] (#720).
///
/// A dangling index is a structural defect, whatever its class: nothing computed it from a block of
/// this product, and the next block given that name would inherit it. Linear in the block count.
pub fn check_chunk_indexes_resolve(blocks: &[BlockRef]) -> crate::Result<()> {
    let names: std::collections::HashSet<&str> = blocks.iter().map(|b| b.name.as_str()).collect();
    for b in blocks.iter().filter(|b| b.is_chunk_index()) {
        let target = b.spec.get("indexes").and_then(|v| v.as_str());
        if !target.is_some_and(|t| t != b.name.as_str() && names.contains(t)) {
            return Err(crate::Error::DanglingChunkIndex {
                index: b.name.to_string(),
                indexes: target.map(str::to_owned),
            });
        }
    }
    Ok(())
}

/// The structural rules over one product's block list that every writer and every reader applies,
/// before any digest is looked up: unique names ([`check_unique_names`]), then chunk indexes that
/// resolve ([`check_chunk_indexes_resolve`]).
pub fn check_block_refs(blocks: &[BlockRef]) -> crate::Result<()> {
    check_unique_names(blocks.iter().map(|b| &b.name))?;
    check_chunk_indexes_resolve(blocks)
}

/// The chunk index of the data block `data_name` that a reader may take aggregates from: the block
/// named [`cidx_name`]`(data_name)`, of kind [`BlockKind::ChunkIndex`], **derived**, whose spec says
/// it `indexes` `data_name`. `None` otherwise, and the reader computes from the data, as it does for
/// an unindexed block.
///
/// Derived is the writer's mark (ADR-0028 §4): only a writer computing the index from the block lands
/// one derived, and `tessera commit` never copies one (#720). A primary block that looks like an
/// index is data someone supplied; its statistics are a claim about the block, not a function of it,
/// so it is never consulted. A reader still checks the index against the block's shape before use.
pub fn trusted_chunk_index<'a>(blocks: &'a [BlockRef], data_name: &str) -> Option<&'a BlockRef> {
    let name = cidx_name(data_name);
    blocks.iter().find(|b| b.name == name).filter(|b| {
        b.kind == BlockKind::ChunkIndex
            && !b.is_primary()
            && b.spec.get("indexes").and_then(|v| v.as_str()) == Some(data_name)
    })
}

/// The name of the chunk-index sidecar (`.cidx`, SPEC §5d) of the data block `data_name`: the sibling
/// `<data_name>.cidx`. The one naming rule its writer
/// (`tessera_io::chunk_index::chunk_index_block`) and [`derived_family`] share.
pub fn cidx_name(data_name: &str) -> String {
    format!("{data_name}.cidx")
}

/// The name of level `level` of a multiscale pyramid of `base`: `<base>/<level>`, the level in
/// decimal without leading zeros (`volume/0` is full resolution and primary; `volume/1`, … are its
/// derived downsamples). The one naming rule `tessera pyramid` and [`derived_family`] share.
pub fn pyramid_level_name(base: &str, level: u32) -> String {
    format!("{base}/{level}")
}

/// `name` as a pyramid level (`<base>/<level>`, as [`pyramid_level_name`] writes it): its base and
/// level, or `None` when the last segment is not a level number in that one spelling.
fn pyramid_level(name: &str) -> Option<(&str, u32)> {
    let (base, last) = name.rsplit_once('/')?;
    let level: u32 = last.parse().ok()?;
    (pyramid_level_name(base, level) == name).then_some((base, level))
}

/// The blocks of `blocks` that are derived from the block `name`, by the naming rules their writers
/// use, so they go when `name` is removed (`tessera commit --remove-block`, ADR-0028 §4):
///
/// - its chunk-index sidecar [`cidx_name`]`(name)`, when **derived**;
/// - when `name` is level 0 of a pyramid (`<base>/0`), every **derived** level `<base>/<n>`,
///   `n ≥ 1`, and the derived sidecar of each.
///
/// Only derived blocks: a primary block of one of those names is data of its own, not something
/// computed from `name`. A primary `name.cidx` stays, and no reader takes aggregates from it
/// ([`trusted_chunk_index`]), so the next block of that name inherits nothing from it (#347, #720).
///
/// Without this, removing level 0 left its downsamples as the product's only blocks: real data under
/// the empty set's `content_hash`. In manifest order; `name` itself is not listed.
pub fn derived_family<'a>(name: &str, blocks: &'a [BlockRef]) -> Vec<&'a BlockName> {
    let cidx = cidx_name(name);
    let pyramid_base = match pyramid_level(name) {
        Some((base, 0)) => Some(base),
        _ => None,
    };
    let is_level = |n: &str| {
        pyramid_base.is_some_and(
            |base| matches!(pyramid_level(n), Some((b, level)) if b == base && level > 0),
        )
    };
    let derived_levels: Vec<&str> = blocks
        .iter()
        .filter(|b| !b.is_primary() && is_level(b.name.as_str()))
        .map(|b| b.name.as_str())
        .collect();
    blocks
        .iter()
        .filter(|b| !b.is_primary())
        .filter(|b| {
            let n = b.name.as_str();
            n == cidx
                || derived_levels.contains(&n)
                || derived_levels.iter().any(|level| n == cidx_name(level))
        })
        .map(|b| &b.name)
        .collect()
}

/// The product's `content_hash` over `blocks` (SPEC §4, ADR-0028 §4): the Merkle root of the
/// [primary](BlockClass::Primary) blocks' digests, in manifest order. Derived blocks are skipped; a
/// list with no primary block (empty, or derived blocks only) gives the root of the empty set.
///
/// # Errors
/// [`crate::Error::MissingDigest`] for the first block, of either class, without a digest: a sealed
/// product's blocks all carry one (SPEC §5).
pub fn content_root<'a>(blocks: impl IntoIterator<Item = &'a BlockRef>) -> crate::Result<String> {
    let mut leaves = Vec::new();
    for b in blocks {
        let d = b
            .digest
            .as_ref()
            .ok_or_else(|| crate::Error::MissingDigest(b.name.to_string()))?;
        if b.is_primary() {
            leaves.push(d.clone());
        }
    }
    Ok(crate::hash::merkle_root(&leaves))
}

/// Behaviour every block backend provides to the product builder.
pub trait Block {
    fn name(&self) -> &str;
    fn kind(&self) -> BlockKind;
    /// JSON descriptor of this block's storage spec (embedded in the manifest).
    fn spec_json(&self) -> crate::Result<serde_json::Value>;
    /// Content digest over this block's encoded spec/payload.
    fn digest(&self) -> crate::Result<String>;
    /// Build the manifest reference for this block.
    fn block_ref(&self) -> crate::Result<BlockRef> {
        Ok(BlockRef {
            name: BlockName::new(self.name())?,
            kind: self.kind(),
            digest: Some(self.digest()?),
            class: BlockClass::Primary,
            spec: self.spec_json()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn block(name: &str, digest: &str, class: BlockClass) -> BlockRef {
        BlockRef {
            name: BlockName::new(name).unwrap(),
            kind: BlockKind::Array,
            digest: Some(digest.into()),
            class,
            spec: json!({}),
        }
    }

    /// A primary block writes no `class` key (so products without derived blocks keep their bytes),
    /// a derived one writes `"class":"derived"`, and both read back as written.
    #[test]
    fn the_class_has_one_spelling_and_round_trips() {
        let primary = block("v", "blake3:00", BlockClass::Primary);
        let v = serde_json::to_value(&primary).unwrap();
        assert!(v.get("class").is_none(), "{v}");
        assert_eq!(serde_json::from_value::<BlockRef>(v).unwrap(), primary);

        let derived = primary.clone().derived();
        let v = serde_json::to_value(&derived).unwrap();
        assert_eq!(v["class"], "derived");
        assert_eq!(serde_json::from_value::<BlockRef>(v).unwrap(), derived);
        assert!(!derived.is_primary());
    }

    /// Any `class` value but `"derived"` is refused, `"primary"` and `null` included: the absent key
    /// is the only spelling of a primary block.
    #[test]
    fn any_other_class_value_is_refused() {
        for bad in [
            json!("primary"),
            json!(null),
            json!("Derived"),
            json!("canonical"),
            json!(""),
            json!(1),
            json!(true),
            json!(["derived"]),
            json!({"derived": null}),
        ] {
            let v = json!({"name": "v", "kind": "array", "digest": "blake3:00", "class": bad, "spec": {}});
            assert!(
                serde_json::from_value::<BlockRef>(v).is_err(),
                "class {bad} must be refused"
            );
        }
        assert!(serde_json::to_value(BlockClass::Primary).is_err());
    }

    /// Removing a block takes its derived family with it: its derived `.cidx`, and for pyramid level
    /// 0 every derived level and their derived sidecars, never a primary block of one of those names
    /// (a `.cidx` or a level), a non-canonical level spelling, another base's levels, or a partition
    /// leaf.
    #[test]
    fn derived_family_follows_the_writers_naming_rules() {
        let d = BlockClass::Derived;
        let p = BlockClass::Primary;
        let blocks = [
            block("volume/0", "blake3:00", p),
            block("volume/0.cidx", "blake3:01", d),
            block("volume/1", "blake3:02", d),
            block("volume/1.cidx", "blake3:03", d),
            block("volume/2", "blake3:04", d),
            block("volume/2.cidx", "blake3:0a", p), // primary: data of its own, not an index of /2
            block("volume/3", "blake3:05", p),      // primary: data of its own
            block("ct", "blake3:0b", p),
            block("ct.cidx", "blake3:0c", p), // primary: data of its own, not an index of ct
            block("volume/02", "blake3:06", d), // not a level spelling the writer uses
            block("other/1", "blake3:07", d),
            block("pairs/0000", "blake3:08", p),
            block("pairs/0001", "blake3:09", d),
        ];
        let names = |name: &str| -> Vec<&str> {
            derived_family(name, &blocks)
                .iter()
                .map(|n| n.as_str())
                .collect()
        };
        assert_eq!(
            names("volume/0"),
            ["volume/0.cidx", "volume/1", "volume/1.cidx", "volume/2"]
        );
        assert_eq!(names("volume/1"), ["volume/1.cidx"]);
        assert!(names("volume/3").is_empty());
        assert!(
            names("volume/2").is_empty(),
            "a primary .cidx is no sidecar"
        );
        assert!(names("ct").is_empty(), "a primary .cidx is no sidecar");
        assert!(
            names("pairs/0000").is_empty(),
            "a partition leaf is no pyramid level 0"
        );
        assert!(names("absent").is_empty());
        assert_eq!(cidx_name("volume/0"), "volume/0.cidx");
        assert_eq!(pyramid_level_name("volume", 2), "volume/2");
    }

    /// A reader takes aggregates only from a derived chunk-index block named for, and declaring, the
    /// block; `is_chunk_index` knows an index by its kind or its `indexes` key, never by its name.
    #[test]
    fn only_a_derived_index_of_the_block_is_trusted() {
        let index = |name: &str, kind: BlockKind, class: BlockClass, indexes: &str| BlockRef {
            name: BlockName::new(name).unwrap(),
            kind,
            digest: Some("blake3:00".into()),
            class,
            spec: json!({"recipe": "chunk_index@1", "indexes": indexes}),
        };
        let (d, p, ci) = (
            BlockClass::Derived,
            BlockClass::Primary,
            BlockKind::ChunkIndex,
        );
        let good = index("volume.cidx", ci, d, "volume");
        assert_eq!(
            trusted_chunk_index(std::slice::from_ref(&good), "volume"),
            Some(&good)
        );
        for (bad, why) in [
            (index("volume.cidx", ci, p, "volume"), "primary"),
            (index("volume.cidx", ci, d, "ct"), "indexes another block"),
            (
                index("volume.cidx", BlockKind::Array, d, "volume"),
                "not a chunk index",
            ),
            (index("volume.idx", ci, d, "volume"), "not the sidecar name"),
        ] {
            assert_eq!(trusted_chunk_index(&[bad], "volume"), None, "{why}");
        }

        assert!(good.is_chunk_index());
        assert!(index("notes", ci, p, "volume").is_chunk_index(), "by kind");
        assert!(
            index("notes", BlockKind::Array, p, "volume").is_chunk_index(),
            "by schema"
        );
        let mut plain = block("volume.cidx", "blake3:00", p);
        assert!(!plain.is_chunk_index(), "a name is not an index");
        plain.spec = json!({"dtype": "int16"});
        assert!(!plain.is_chunk_index());
    }

    /// `content_root` folds the primary digests in manifest order, skips derived ones, gives the
    /// empty root for derived blocks only, and refuses a block of either class without a digest.
    #[test]
    fn content_root_is_over_the_primary_blocks_in_order() {
        let (a, b) = ("blake3:aa".to_string(), "blake3:bb".to_string());
        let blocks = [
            block("a", &a, BlockClass::Primary),
            block("a.cidx", "blake3:cc", BlockClass::Derived),
            block("b", &b, BlockClass::Primary),
        ];
        assert_eq!(
            content_root(&blocks).unwrap(),
            crate::hash::merkle_root(&[a.clone(), b.clone()])
        );
        assert_ne!(
            content_root(&blocks).unwrap(),
            crate::hash::merkle_root(&[b, a]),
            "manifest order"
        );
        assert_eq!(
            content_root(&blocks[1..2]).unwrap(),
            crate::hash::merkle_root(&[])
        );
        let mut undigested = blocks.clone();
        undigested[1].digest = None;
        assert!(matches!(
            content_root(&undigested),
            Err(crate::Error::MissingDigest(n)) if n == "a.cidx"
        ));
    }
}
