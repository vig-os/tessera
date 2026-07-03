//! `NodeTree` — the structural hierarchy of a `.tsra`, as a presentation-agnostic view-model.
//!
//! A `.tsra` browses like a self-describing zarr group: the product is the root, its metadata fields
//! are attributes, each block is an array / (multi-block) table / blob whose columns are leaves, and
//! `sources` · `extra` · `aux` hang off the spine. This module turns a [`Manifest`] (+ the container's
//! aux member names) into a [`NodeTree`] of typed [`Node`]s — the **one** structural view-model behind
//! the CLI `tree`, the TUI navigator, the MCP `tsra_tree` tool, and a future `serve` (all thin
//! renderers over this, per `docs/spikes/tsra-explorer-wireframes.md` § Traceability).
//!
//! It is deliberately *structural*, not *judgmental*: it says "there is a `schema` node for product
//! `pet-ct v1`", never "schema ✓ conformant". Integrity / signature / conformance verdicts are the
//! separate `ArtifactVerdict` view-model (the Verify wireframe). Keeping the two apart means the
//! navigator can render instantly from the manifest without decoding or re-hashing anything.
//!
//! Filesystem-adjacent artifacts (detached `.sig.json` sidecars, `.fcrypt.json` envelopes) are **not**
//! here — they are a `&Path`-on-disk concern the CLI overlays, not part of the product itself, so the
//! view-model stays Reader-generic (local + `cloud`) per the crate's design invariants.

use serde::Serialize;
use serde_json::Value;
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::Manifest;

/// The structural hierarchy of one `.tsra` product — a single [`Node`] root whose children are the
/// spine sections (`meta` · `schema` · `referencing` · `blocks` · `sources` · `extra` · `aux`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeTree {
    /// The product node; its `children` are the present spine sections, in canonical order.
    pub root: Node,
}

/// One node in the [`NodeTree`]: a display `label`, a glanceable `detail` annotation, a typed [`kind`]
/// (drives the renderer's glyph and which mode a TUI opens on select), an optional [`handle`] naming
/// what the node addresses for drill-down, and its `children`.
///
/// [`kind`]: Node::kind
/// [`handle`]: Node::handle
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    /// Display name — `"meta"`, a block name (`"pet_suv"`), a metadata key, a column name.
    pub label: String,
    /// Short right-aligned annotation — a count (`"4"`), a block headline (`"array  f32  [200, 200, 402]  pcodec"`),
    /// a field tier/sensitivity (`"required · identifying"`), a compact value. Renderers may elide it;
    /// it is stored untruncated so each renderer picks its own width.
    pub detail: String,
    /// The structural category — the renderer maps it to a glyph, and a TUI to an open action.
    pub kind: NodeKind,
    /// What this node addresses, when it is drillable into a content view (a block to stat/read, an
    /// `extra`/`aux` member to dump, a provenance edge). Groups and inline leaves carry `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<NodeHandle>,
    /// Child nodes (a group's members, a block's columns / spec detail). Empty for a plain leaf.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

/// The structural category of a [`Node`] — a closed set covering every spine position. Renderers match
/// on it for icons/colours; a TUI maps it to the mode a node opens (a `Block` → Data, a `MetaField` →
/// Inspect). `Block` carries its [`BlockKind`] so the glyph and Data-mode dispatch follow the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// The product root.
    Product,
    /// The `meta` grouping node.
    MetaGroup,
    /// One metadata field (schema-governed value).
    MetaField,
    /// The `schema` grouping node (the embedded, self-describing product schema).
    SchemaGroup,
    /// One declared schema field (id + tier + sensitivity).
    SchemaField,
    /// A synthesized `referencing` node — world/affine addressing summarised from an array block.
    Referencing,
    /// The `blocks` grouping node.
    BlockGroup,
    /// A storage block; carries its [`BlockKind`] for the glyph and Data-mode dispatch.
    Block(BlockKind),
    /// A table column leaf under a `Block`.
    Column,
    /// Non-column block spec detail (an array's `chunks …`, a blob's `file …`).
    BlockDetail,
    /// The `sources` grouping node.
    SourceGroup,
    /// One provenance edge.
    Source,
    /// The `extra` grouping node (the fd5 extension namespace).
    ExtraGroup,
    /// One `extra/<key>` extension field.
    ExtraField,
    /// The `aux` grouping node (non-sealed members carried inside the container, ADR-0042).
    AuxGroup,
    /// One `aux/<name>` member.
    AuxMember,
}

/// A stable address for a drillable [`Node`] — what a content view / MCP tool / serve endpoint uses to
/// fetch that node's data. Distinct from the display `label`: identity, not presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum NodeHandle {
    /// A storage block, by name — the key for `stats` / `read` / `slice` / `project`.
    Block(String),
    /// A column within a (possibly multi-block logical) table.
    Column {
        /// The owning table block's name.
        block: String,
        /// The column name.
        column: String,
    },
    /// An `extra/<key>` extension field, by key.
    Extra(String),
    /// An `aux/<name>` embedded member, by name.
    Aux(String),
    /// A provenance edge, by its index in `manifest.sources`.
    Source(usize),
}

/// Build the structural [`NodeTree`] for a product from its [`Manifest`] and the container's `aux`
/// member names (pull those from `Reader::aux_names()` — they live in the zip directory, not the
/// manifest). Pure and cheap: no payload decode, no hashing, no I/O.
pub fn hierarchy(manifest: &Manifest, aux: &[String]) -> NodeTree {
    let m = manifest;
    let mut children = Vec::new();

    // meta — the schema-governed metadata fields.
    if !m.metadata.is_empty() {
        let kids = m
            .metadata
            .iter()
            .map(|(k, v)| Node::leaf(k, compact_value(v), NodeKind::MetaField))
            .collect();
        children.push(Node::group(
            "meta",
            m.metadata.len(),
            NodeKind::MetaGroup,
            kids,
        ));
    }

    // schema — the embedded, self-describing product schema (its declared field roster as leaves).
    if let Some(s) = m
        .schema
        .as_ref()
        .and_then(|v| tessera_core::ProductSchema::from_value(v).ok())
    {
        let kids = s
            .fields
            .iter()
            .map(|f| {
                let tier = field_tier(f.required, f.recommended);
                let sens = format!("{:?}", f.sensitivity).to_lowercase();
                Node::leaf(&f.id, format!("{tier} · {sens}"), NodeKind::SchemaField)
            })
            .collect();
        let mut node = Node::group("schema", s.fields.len(), NodeKind::SchemaGroup, kids);
        node.detail = format!("{} v{}", s.product, s.version);
        children.push(node);
    }

    // referencing — world/affine addressing, summarised from the first array block that carries a
    // world frame. A synthesized structural node (there is no manifest section for it): it tells the
    // navigator whether the product is spatially addressable and in which convention/unit.
    if let Some(detail) = referencing_summary(&m.blocks) {
        children.push(Node {
            label: "referencing".into(),
            detail,
            kind: NodeKind::Referencing,
            handle: None,
            children: Vec::new(),
        });
    }

    // blocks — every storage block, with its shape headline + columns / spec detail.
    if !m.blocks.is_empty() {
        let kids = m.blocks.iter().map(block_node).collect();
        children.push(Node::group(
            "blocks",
            m.blocks.len(),
            NodeKind::BlockGroup,
            kids,
        ));
    }

    // sources — the provenance DAG edges (role <- reference).
    if !m.sources.is_empty() {
        let kids = m
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut node =
                    Node::leaf(&s.role, compact_reference(&s.reference), NodeKind::Source);
                node.handle = Some(NodeHandle::Source(i));
                node
            })
            .collect();
        children.push(Node::group(
            "sources",
            m.sources.len(),
            NodeKind::SourceGroup,
            kids,
        ));
    }

    // extra — the fd5 extension namespace (vendor/provenance blobs; e.g. the full DICOM header).
    if !m.extra.is_empty() {
        let kids = m
            .extra
            .iter()
            .map(|(k, v)| {
                let mut node = Node::leaf(k, value_kind(v), NodeKind::ExtraField);
                node.handle = Some(NodeHandle::Extra(k.clone()));
                node
            })
            .collect();
        children.push(Node::group(
            "extra",
            m.extra.len(),
            NodeKind::ExtraGroup,
            kids,
        ));
    }

    // aux — non-sealed members carried inside the container (ADR-0042): embedded signature, ingest
    // provenance, anything future producers stamp.
    if !aux.is_empty() {
        let kids = aux
            .iter()
            .map(|n| {
                let mut node = Node::leaf(n, String::new(), NodeKind::AuxMember);
                node.handle = Some(NodeHandle::Aux(n.clone()));
                node
            })
            .collect();
        children.push(Node::group("aux", aux.len(), NodeKind::AuxGroup, kids));
    }

    NodeTree {
        root: Node {
            label: m.name.clone(),
            detail: m.product.clone(),
            kind: NodeKind::Product,
            handle: None,
            children,
        },
    }
}

impl Node {
    /// A leaf node (no children, no handle) with the given label/detail/kind.
    fn leaf(label: &str, detail: impl Into<String>, kind: NodeKind) -> Node {
        Node {
            label: label.to_string(),
            detail: detail.into(),
            kind,
            handle: None,
            children: Vec::new(),
        }
    }

    /// A grouping node whose `detail` is the child count; `kind` is the group kind.
    fn group(label: &str, count: usize, kind: NodeKind, children: Vec<Node>) -> Node {
        Node {
            label: label.to_string(),
            detail: count.to_string(),
            kind,
            handle: None,
            children,
        }
    }
}

/// One storage block as a [`Node`]: label = block name, detail = shape headline, handle = the block
/// name, children = its columns (tables) or spec detail (arrays/blobs).
fn block_node(b: &BlockRef) -> Node {
    Node {
        label: b.name.clone(),
        detail: block_headline(&b.kind, &b.spec),
        kind: NodeKind::Block(b.kind),
        handle: Some(NodeHandle::Block(b.name.clone())),
        children: block_children(&b.name, &b.kind, &b.spec),
    }
}

/// `required` / `recommended` / `optional` from a schema field's two flags.
fn field_tier(required: bool, recommended: bool) -> &'static str {
    if required {
        "required"
    } else if recommended {
        "recommended"
    } else {
        "optional"
    }
}

/// The `convention unit` of the first array block that carries a world frame (`"LPS mm"`), or `None`
/// if no block is spatially referenced (index-space only).
fn referencing_summary(blocks: &[BlockRef]) -> Option<String> {
    blocks.iter().find_map(|b| {
        let wf = b.spec.get("world_frame")?;
        let conv = wf.get("convention").and_then(Value::as_str).unwrap_or("?");
        let unit = wf.get("unit").and_then(Value::as_str).unwrap_or("");
        Some(format!("{conv} {unit}").trim().to_string())
    })
}

/// One-line block summary (`array  int16  [128, 512, 512]  pcodec` / `table  6 cols × 4,194,304 rows`).
/// Shared with the CLI text renderers (`tessera-cli::nav`) so the one-liner never drifts between the
/// structural view-model and the `tree`/`ls` output.
pub fn block_headline(kind: &BlockKind, spec: &Value) -> String {
    match kind {
        BlockKind::Array => {
            let dtype = spec.get("dtype").and_then(Value::as_str).unwrap_or("?");
            let codec = spec.get("codec").and_then(Value::as_str).unwrap_or("?");
            format!("array  {dtype}  {}  {codec}", shape_str(spec))
        }
        BlockKind::Table => {
            let ncols = spec
                .get("columns")
                .and_then(Value::as_array)
                .map(|c| c.len())
                .unwrap_or(0);
            let rows = spec.get("rows").and_then(Value::as_u64).unwrap_or(0);
            format!("table  {ncols} cols × {} rows", thousands(rows))
        }
        BlockKind::ChunkIndex => "index  (per-chunk hash + stats)".to_string(),
        BlockKind::Blob => {
            let mt = spec
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let size = spec.get("size").and_then(Value::as_u64).unwrap_or(0);
            format!("blob   {} · {mt}", human_bytes(size))
        }
    }
}

/// Child nodes of a block: `Column` leaves (`name  dtype`) for tables, a spec-detail leaf for
/// arrays (`chunks [..]`) / blobs (`file …`).
fn block_children(block: &str, kind: &BlockKind, spec: &Value) -> Vec<Node> {
    match kind {
        BlockKind::Array => {
            let chunks = spec
                .get("chunks")
                .and_then(Value::as_array)
                .map(|c| format!("chunks {}", dims_str(c)))
                .unwrap_or_else(|| "chunks [?]".into());
            vec![Node::leaf("chunks", chunks, NodeKind::BlockDetail)]
        }
        BlockKind::Table => spec
            .get("columns")
            .and_then(Value::as_array)
            .map(|cols| {
                cols.iter()
                    .map(|c| {
                        let n = c.get("name").and_then(Value::as_str).unwrap_or("?");
                        let d = c.get("dtype").and_then(Value::as_str).unwrap_or("?");
                        let mut node = Node::leaf(n, d.to_string(), NodeKind::Column);
                        node.handle = Some(NodeHandle::Column {
                            block: block.to_string(),
                            column: n.to_string(),
                        });
                        node
                    })
                    .collect()
            })
            .unwrap_or_default(),
        BlockKind::ChunkIndex => Vec::new(),
        BlockKind::Blob => spec
            .get("filename")
            .and_then(Value::as_str)
            .map(|f| vec![Node::leaf("file", f.to_string(), NodeKind::BlockDetail)])
            .unwrap_or_default(),
    }
}

/// Render a JSON int array as `[128, 512, 512]`.
fn dims_str(dims: &[Value]) -> String {
    let parts: Vec<String> = dims
        .iter()
        .map(|d| {
            d.as_u64()
                .map(|u| u.to_string())
                .unwrap_or_else(|| "?".into())
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// Render `spec["shape"]` (a JSON int array) as `[128, 512, 512]`.
pub fn shape_str(spec: &Value) -> String {
    match spec.get("shape").and_then(Value::as_array) {
        Some(dims) => dims_str(dims),
        None => "[?]".to_string(),
    }
}

/// Group-of-three thousands separators for human row counts (`4194304` → `4,194,304`).
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let first = bytes.len() % 3;
    for (i, b) in bytes.iter().enumerate() {
        if i != 0 && i >= first && (i - first).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Human-readable byte size (`3.0 GiB`, `512 KiB`) for a blob block.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Compact one-line render of a metadata JSON value (untruncated — renderers elide to their width).
fn compact_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

/// A one-word shape summary of an `extra` value (`object, 12 keys` / `array, 3 items` / `string`).
fn value_kind(v: &Value) -> String {
    match v {
        Value::Object(o) => format!("object, {} keys", o.len()),
        Value::Array(a) => format!("array, {} items", a.len()),
        Value::String(_) => "string".into(),
        other => other.to_string(),
    }
}

/// Compact render of a provenance-edge reference: a comma-joined list (a DICOM series' many slice
/// paths) collapses to `<first> (+N more)` so the navigator never floods with hundreds of paths.
fn compact_reference(reference: &str) -> String {
    match reference.split_once(',') {
        Some((first, rest)) => {
            let more = rest.split(',').filter(|s| !s.trim().is_empty()).count();
            format!("{first} (+{more} more)")
        }
        None => reference.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tessera_core::block::BlockRef;
    use tessera_core::Manifest;

    /// A manifest with a metadata field, a table block (2 columns), an array block with a world
    /// frame, a provenance edge, and one `extra` key — exercises every section builder.
    fn rich_manifest() -> Manifest {
        let mut m = Manifest::new("pet-ct", "study-014", "d", "2024-01-01T00:00:00Z");
        m.metadata.insert("modality".into(), json!("PT"));
        m.blocks.push(BlockRef {
            name: "events".into(),
            kind: BlockKind::Table,
            digest: Some("blake3:aa".into()),
            spec: json!({
                "columns": [{"name": "ms", "dtype": "u4"}, {"name": "en", "dtype": "f4"}],
                "rows": 4_194_304u64
            }),
        });
        m.blocks.push(BlockRef {
            name: "pet_suv".into(),
            kind: BlockKind::Array,
            digest: Some("blake3:bb".into()),
            spec: json!({
                "dtype": "f32", "codec": "pcodec",
                "shape": [200, 200, 402], "chunks": [64, 64, 64],
                "world_frame": {"convention": "LPS", "unit": "mm"}
            }),
        });
        m.sources.push(tessera_core::provenance::Source::new(
            "ingested_from",
            "/data/a.dcm,/data/b.dcm",
        ));
        m.extra
            .insert("dicom_header".into(), json!({"x": 1, "y": 2}));
        m
    }

    #[test]
    fn hierarchy_covers_every_spine_section_in_canonical_order() {
        let t = hierarchy(&rich_manifest(), &[]);
        assert_eq!(t.root.kind, NodeKind::Product);
        assert_eq!(t.root.label, "study-014");
        assert_eq!(t.root.detail, "pet-ct");
        let sections: Vec<&NodeKind> = t.root.children.iter().map(|n| &n.kind).collect();
        assert_eq!(
            sections,
            vec![
                &NodeKind::MetaGroup,
                &NodeKind::Referencing,
                &NodeKind::BlockGroup,
                &NodeKind::SourceGroup,
                &NodeKind::ExtraGroup,
            ],
            "sections present + ordered (no schema here; aux folds in only when named)"
        );
    }

    #[test]
    fn referencing_summarises_the_array_world_frame() {
        let t = hierarchy(&rich_manifest(), &[]);
        let r = t
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::Referencing)
            .unwrap();
        assert_eq!(r.detail, "LPS mm");
    }

    #[test]
    fn table_block_exposes_columns_with_drill_handles() {
        let t = hierarchy(&rich_manifest(), &[]);
        let blocks = t
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::BlockGroup)
            .unwrap();
        let events = blocks
            .children
            .iter()
            .find(|n| n.label == "events")
            .unwrap();
        assert_eq!(events.kind, NodeKind::Block(BlockKind::Table));
        assert_eq!(events.detail, "table  2 cols × 4,194,304 rows");
        assert_eq!(events.handle, Some(NodeHandle::Block("events".into())));
        let ms = &events.children[0];
        assert_eq!(ms.kind, NodeKind::Column);
        assert_eq!(
            ms.handle,
            Some(NodeHandle::Column {
                block: "events".into(),
                column: "ms".into()
            })
        );
    }

    #[test]
    fn array_block_headline_and_chunks_leaf() {
        let t = hierarchy(&rich_manifest(), &[]);
        let blocks = t
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::BlockGroup)
            .unwrap();
        let suv = blocks
            .children
            .iter()
            .find(|n| n.label == "pet_suv")
            .unwrap();
        assert_eq!(suv.kind, NodeKind::Block(BlockKind::Array));
        assert_eq!(suv.detail, "array  f32  [200, 200, 402]  pcodec");
        assert_eq!(suv.children[0].detail, "chunks [64, 64, 64]");
    }

    #[test]
    fn sources_collapse_a_multi_path_reference() {
        let t = hierarchy(&rich_manifest(), &[]);
        let src = t
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::SourceGroup)
            .unwrap();
        let edge = &src.children[0];
        assert_eq!(edge.label, "ingested_from");
        assert_eq!(edge.detail, "/data/a.dcm (+1 more)");
        assert_eq!(edge.handle, Some(NodeHandle::Source(0)));
    }

    #[test]
    fn extra_and_aux_carry_dump_handles() {
        let t = hierarchy(&rich_manifest(), &["provenance.json".to_string()]);
        let extra = t
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::ExtraGroup)
            .unwrap();
        assert_eq!(extra.children[0].label, "dicom_header");
        assert_eq!(extra.children[0].detail, "object, 2 keys");
        assert_eq!(
            extra.children[0].handle,
            Some(NodeHandle::Extra("dicom_header".into()))
        );
        let aux = t
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::AuxGroup)
            .unwrap();
        assert_eq!(
            aux.children[0].handle,
            Some(NodeHandle::Aux("provenance.json".into()))
        );
    }

    #[test]
    fn serializes_to_json_for_the_mcp_and_serve_surfaces() {
        let t = hierarchy(&rich_manifest(), &[]);
        let v = serde_json::to_value(&t).unwrap();
        // Root shape + a tagged handle round-trips as structured JSON (not rendered text).
        assert_eq!(v["root"]["kind"], "product");
        let blocks = v["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["kind"] == "block_group")
            .unwrap();
        let events = blocks["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["label"] == "events")
            .unwrap();
        assert_eq!(events["handle"]["kind"], "block");
        assert_eq!(events["handle"]["id"], "events");
    }
}
