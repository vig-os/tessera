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
