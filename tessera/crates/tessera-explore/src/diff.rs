//! `ManifestDiff` — a field-level comparison of two products' manifests, for the Compare pane (A/B or
//! a version against its prior). Presentation-agnostic + `Serialize` (the CLI / an MCP tool / `serve`
//! render the same rows), computed from the two manifests alone — no payload decode, so comparing two
//! sealed `.tsra` is cheap and offline.
//!
//! It compares the *identity/seal*, each *block* (by name → digest), and each *metadata* field (by
//! key → value), classifying every row as unchanged / changed / added-in-B / removed-from-B. Block
//! payloads are compared by their recorded digests (the seal already binds bytes to digest), so a
//! digest change *is* a byte change — no need to read the blocks.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;
use tessera_core::Manifest;

/// A field-level diff of two manifests as an ordered list of [`DiffRow`]s (identity, then blocks, then
/// metadata).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManifestDiff {
    /// One row per compared field, in a stable order.
    pub rows: Vec<DiffRow>,
}

impl ManifestDiff {
    /// How many rows actually differ (changed / added / removed) — the headline count.
    pub fn changed(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.status != DiffStatus::Same)
            .count()
    }
}

/// One compared field: its name, the A and B values (`—` when absent on a side), and the verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffRow {
    /// Field name — `product` · `id` · `manifest_hash` · `block:<name>` · `meta:<key>`.
    pub field: String,
    /// Value on side A (`—` when the field is absent in A).
    pub a: String,
    /// Value on side B (`—` when the field is absent in B).
    pub b: String,
    /// The comparison verdict.
    pub status: DiffStatus,
}

/// The verdict for a [`DiffRow`] — from B's perspective relative to A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffStatus {
    /// Present on both sides with equal values.
    Same,
    /// Present on both sides with different values.
    Changed,
    /// Present in B, absent in A.
    Added,
    /// Present in A, absent in B.
    Removed,
}

/// Compare two manifests field-by-field into a [`ManifestDiff`] (`a` = the base / prior, `b` = the
/// other / newer).
pub fn manifest_diff(a: &Manifest, b: &Manifest) -> ManifestDiff {
    let mut rows = Vec::new();

    // Identity + seal. The id / manifest_hash are long content hashes — shorten them to a glanceable
    // prefix (a prefix mismatch already tells you they differ; the full value lives in the manifest).
    rows.push(scalar("product", &a.product, &b.product));
    rows.push(scalar("id", &short_hash(&a.id), &short_hash(&b.id)));
    rows.push(scalar(
        "manifest_hash",
        &a.manifest_hash
            .as_deref()
            .map(short_hash)
            .unwrap_or_else(|| "—".into()),
        &b.manifest_hash
            .as_deref()
            .map(short_hash)
            .unwrap_or_else(|| "—".into()),
    ));

    // Blocks — union of names, compared by recorded digest (a digest change is a byte change).
    let block_names: BTreeSet<&str> = a
        .blocks
        .iter()
        .chain(&b.blocks)
        .map(|x| x.name.as_str())
        .collect();
    for name in block_names {
        let da = a
            .blocks
            .iter()
            .find(|x| x.name == name)
            .and_then(|x| x.digest.as_deref())
            .map(short_hash);
        let db = b
            .blocks
            .iter()
            .find(|x| x.name == name)
            .and_then(|x| x.digest.as_deref())
            .map(short_hash);
        rows.push(optional(&format!("block:{name}"), da, db));
    }

    // Metadata — union of keys, compared by compact value.
    let keys: BTreeSet<&str> = a
        .metadata
        .keys()
        .chain(b.metadata.keys())
        .map(String::as_str)
        .collect();
    for k in keys {
        let va = a.metadata.get(k).map(compact_value);
        let vb = b.metadata.get(k).map(compact_value);
        rows.push(optional(&format!("meta:{k}"), va, vb));
    }

    ManifestDiff { rows }
}

/// A row for a field present on both sides (Same / Changed).
fn scalar(field: &str, a: &str, b: &str) -> DiffRow {
    DiffRow {
        field: field.to_string(),
        a: a.to_string(),
        b: b.to_string(),
        status: if a == b {
            DiffStatus::Same
        } else {
            DiffStatus::Changed
        },
    }
}

/// A row for a field that may be absent on either side (Same / Changed / Added / Removed).
fn optional(field: &str, a: Option<String>, b: Option<String>) -> DiffRow {
    let status = match (&a, &b) {
        (Some(x), Some(y)) if x == y => DiffStatus::Same,
        (Some(_), Some(_)) => DiffStatus::Changed,
        (None, Some(_)) => DiffStatus::Added,
        (Some(_), None) => DiffStatus::Removed,
        (None, None) => DiffStatus::Same, // unreachable (the key came from one side)
    };
    DiffRow {
        field: field.to_string(),
        a: a.unwrap_or_else(|| "—".into()),
        b: b.unwrap_or_else(|| "—".into()),
        status,
    }
}

/// Shorten a `blake3:<hex>` digest to a glanceable prefix for the diff table.
fn short_hash(d: &str) -> String {
    match d.split_once(':') {
        Some((alg, hex)) if hex.len() > 12 => format!("{alg}:{}…", &hex[..12]),
        _ => d.to_string(),
    }
}

/// Compact one-line render of a metadata JSON value.
fn compact_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tessera_core::block::{BlockKind, BlockRef};

    fn manifest(hash: &str) -> Manifest {
        let mut m = Manifest::new("recon", "study", "d", "2024-01-01T00:00:00Z");
        m.manifest_hash = Some(hash.into());
        m
    }

    #[test]
    fn diff_flags_changed_added_and_removed() {
        let mut a = manifest("blake3:aaaa000011112222333344445555");
        a.metadata.insert("tracer".into(), json!("FDG"));
        a.metadata.insert("gone".into(), json!(1));
        a.blocks.push(BlockRef {
            name: "volume".into(),
            kind: BlockKind::Array,
            digest: Some("blake3:1111000011112222333344445555".into()),
            spec: json!({}),
        });

        let mut b = manifest("blake3:bbbb000011112222333344445555");
        b.metadata.insert("tracer".into(), json!("FMISO")); // changed
        b.metadata.insert("new".into(), json!(2)); // added
        b.blocks.push(BlockRef {
            name: "volume".into(),
            kind: BlockKind::Array,
            digest: Some("blake3:9999000011112222333344445555".into()), // changed digest
            spec: json!({}),
        });

        let d = manifest_diff(&a, &b);
        let by = |f: &str| d.rows.iter().find(|r| r.field == f).unwrap().status;
        assert_eq!(by("product"), DiffStatus::Same);
        assert_eq!(by("manifest_hash"), DiffStatus::Changed);
        assert_eq!(by("block:volume"), DiffStatus::Changed);
        assert_eq!(by("meta:tracer"), DiffStatus::Changed);
        assert_eq!(by("meta:new"), DiffStatus::Added);
        assert_eq!(by("meta:gone"), DiffStatus::Removed);
        // product + id are unchanged; the rest differ.
        assert_eq!(d.changed(), 5);
    }

    #[test]
    fn identical_manifests_have_no_changes_and_serialize() {
        let a = manifest("blake3:same00011112222333344445555");
        let d = manifest_diff(&a, &a);
        assert_eq!(d.changed(), 0);
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["rows"][0]["status"], "same");
    }
}
