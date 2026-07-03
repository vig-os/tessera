//! Data-mode loading — turns a selected block into a **render-ready** [`DataView`] by calling the
//! `tessera-explore` view-model (paged table read · array stats + histogram · blob summary) and
//! reducing its Arrow / decoded results to plain strings and numbers. The renderer ([`crate::ui`])
//! stays Arrow-free and pure, so it is snapshot-testable by constructing a [`DataView`] directly; the
//! I/O + decode live here and run only in the event-loop update path, never in render.
//!
//! Every failure (missing block, decode error, non-tabular target) becomes a [`DataView::Unavailable`]
//! with a human reason — Data mode reports it in the pane, never panics.

use std::path::Path;

use tessera_core::block::{BlockKind, BlockRef};
use tessera_explore::array::{array_stats, histogram, ArrayStats};
use tessera_explore::hierarchy::{Node, NodeHandle};
use tessera_explore::table::{page_cells, read_page};
use tessera_io::Reader;

/// Rows materialised per table page (a bounded first-page read; scrolling past this re-reads later).
pub const PAGE_ROWS: u64 = 500;
/// Histogram bin count for the array value distribution.
pub const HIST_BINS: usize = 24;

/// One histogram bar — a half-open value range and its element count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistBin {
    /// Inclusive lower edge of the bin.
    pub lo: f64,
    /// Exclusive upper edge of the bin (inclusive for the last bin).
    pub hi: f64,
    /// Elements falling in this bin.
    pub count: u64,
}

/// A block's data, reduced to exactly what the Data pane renders — no Arrow, no decoded volume held.
#[derive(Debug, Clone, PartialEq)]
pub enum DataView {
    /// A (bounded) page of a table block: header + stringified rows + the true total.
    Table {
        /// Column names in output order.
        columns: Vec<String>,
        /// Stringified cells, row-major (`rows[r][c]`); at most [`PAGE_ROWS`] rows.
        rows: Vec<Vec<String>>,
        /// Total rows in the logical table (may exceed `rows.len()`).
        total: u64,
    },
    /// An array block: shape/dtype/codec, a numeric summary, optional physical rescale, and a
    /// value-distribution histogram.
    Array {
        /// Tessera dtype name (`int16`, `f32`, …).
        dtype: String,
        /// Codec name (`pcodec`, `zstd`, …).
        codec: String,
        /// Array shape.
        shape: Vec<u64>,
        /// Physical unit, when the block declares one.
        unit: Option<String>,
        /// Raw-value summary (min/max/mean/std/count).
        stats: ArrayStats,
        /// `(slope, intercept)` when the block carries a rescale (raw → physical), else `None`.
        rescale: Option<(f64, f64)>,
        /// The (raw-value) histogram bars.
        hist: Vec<HistBin>,
    },
    /// An opaque blob block: media type, byte size, original filename.
    Blob {
        /// Declared media type (`application/octet-stream` default).
        media_type: String,
        /// Byte size of the preserved payload.
        size: u64,
        /// Original filename, when recorded.
        filename: Option<String>,
    },
    /// No data view for the current selection — the reason (e.g. "select a block", or an error).
    Unavailable(String),
}

impl DataView {
    /// Number of loaded rows in a [`DataView::Table`] (0 for any other variant) — the scroll bound.
    pub fn table_len(&self) -> usize {
        match self {
            DataView::Table { rows, .. } => rows.len(),
            _ => 0,
        }
    }

    /// Load the [`DataView`] for the node the navigator has selected. Resolves the node to its owning
    /// block via its [`NodeHandle`] (a block, or a table column's parent table); anything without a
    /// block context yields [`DataView::Unavailable`].
    pub fn load(path: &Path, node: &Node) -> DataView {
        let Some(block) = block_of(node) else {
            return DataView::Unavailable(
                "Select a block in the navigator (1 Navigate), then press 3 to view its data."
                    .into(),
            );
        };
        match load_block(path, &block) {
            Ok(view) => view,
            Err(e) => DataView::Unavailable(format!("could not load '{block}': {e}")),
        }
    }
}

/// The block a Data view should load for a selected node: a block node loads itself; a table column
/// loads its parent table; everything else has no data context. Public so the app can key its cached
/// [`DataView`] on the selected block (reload only when it changes).
pub fn block_of(node: &Node) -> Option<String> {
    match node.handle.as_ref()? {
        NodeHandle::Block(name) => Some(name.clone()),
        NodeHandle::Column { block, .. } => Some(block.clone()),
        _ => None,
    }
}

/// Open the file and build the [`DataView`] for one named block, dispatching on its kind.
fn load_block(path: &Path, block: &str) -> tessera_core::Result<DataView> {
    let mut reader = Reader::open(path)?;
    let bref = reader
        .manifest()
        .blocks
        .iter()
        .find(|b| b.name == block)
        .cloned()
        .ok_or_else(|| tessera_core::Error::Invalid(format!("no block '{block}'")))?;
    match bref.kind {
        BlockKind::Table => load_table(&mut reader, block),
        BlockKind::Array => load_array(&mut reader, block, &bref),
        BlockKind::Blob => Ok(load_blob(&bref)),
        BlockKind::ChunkIndex => Ok(DataView::Unavailable(
            "chunk-index block — a per-chunk hash/stats companion, not directly viewable.".into(),
        )),
    }
}

/// Read a bounded first page of a (logical) table block and stringify it.
fn load_table<R: std::io::Read + std::io::Seek>(
    reader: &mut Reader<R>,
    block: &str,
) -> tessera_core::Result<DataView> {
    let page = read_page(reader, block, &[], |total| (0, total.min(PAGE_ROWS)))?;
    let columns: Vec<String> = page
        .batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    // page_cells is column-major (`cells[col][row]`); transpose to row-major strings for the table.
    let cells = page_cells(&page.batch);
    let nrows = page.batch.num_rows();
    let rows: Vec<Vec<String>> = (0..nrows)
        .map(|r| {
            cells
                .iter()
                .map(|col| cell_string(col.get(r)))
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(DataView::Table {
        columns,
        rows,
        total: page.total,
    })
}

/// Decode an array block and compute its stats + histogram.
fn load_array<R: std::io::Read + std::io::Seek>(
    reader: &mut Reader<R>,
    block: &str,
    bref: &BlockRef,
) -> tessera_core::Result<DataView> {
    let spec: tessera_core::block::array::ArraySpec = serde_json::from_value(bref.spec.clone())
        .map_err(|e| tessera_core::Error::Invalid(format!("bad array spec for '{block}': {e}")))?;
    let blob = reader.read_block(block)?;
    let data = tessera_io::array::decode(&spec, &blob)?;
    let stats = array_stats(&data);
    let rescale = match (spec.rescale_slope, spec.rescale_intercept) {
        (Some(s), Some(i)) => Some((s, i)),
        _ => None,
    };
    let hist_batch = histogram(&data, HIST_BINS, None); // raw-value distribution
    let hist = hist_bins(&hist_batch);
    Ok(DataView::Array {
        dtype: spec.dtype,
        codec: spec.codec,
        shape: spec.shape,
        unit: spec.unit,
        stats,
        rescale,
        hist,
    })
}

/// Summarise a blob block from its manifest spec (no payload read — the bytes are opaque).
fn load_blob(bref: &BlockRef) -> DataView {
    let spec = &bref.spec;
    DataView::Blob {
        media_type: spec
            .get("media_type")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string(),
        size: spec.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
        filename: spec
            .get("filename")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// Extract the `bin_lo` / `bin_hi` / `count` columns of a histogram [`RecordBatch`] into [`HistBin`]s.
fn hist_bins(batch: &arrow::record_batch::RecordBatch) -> Vec<HistBin> {
    use arrow::array::{Float64Array, UInt64Array};
    let lo = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("histogram bin_lo is Float64");
    let hi = batch
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("histogram bin_hi is Float64");
    let count = batch
        .column(2)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("histogram count is UInt64");
    (0..batch.num_rows())
        .map(|i| HistBin {
            lo: lo.value(i),
            hi: hi.value(i),
            count: count.value(i),
        })
        .collect()
}

/// Render one JSON cell as a compact string; a JSON-null (a NaN / ±inf float) shows as `nan`.
fn cell_string(v: Option<&serde_json::Value>) -> String {
    match v {
        None | Some(serde_json::Value::Null) => "nan".to_string(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::{BlockKind, BlockRef};
    use tessera_explore::hierarchy::{hierarchy, NodeKind};

    /// A node with a Block handle resolves to that block; a Column resolves to its parent; a bare
    /// group resolves to nothing.
    #[test]
    fn block_of_resolves_via_the_handle() {
        let mut m = tessera_core::Manifest::new("p", "n", "d", "2024-01-01T00:00:00Z");
        m.blocks.push(BlockRef {
            name: "events".into(),
            kind: BlockKind::Table,
            digest: Some("blake3:aa".into()),
            spec: serde_json::json!({"columns":[{"name":"ms","dtype":"u4"}],"rows":3u64}),
        });
        let tree = hierarchy(&m, &[]);
        let blocks = tree
            .root
            .children
            .iter()
            .find(|n| n.kind == NodeKind::BlockGroup)
            .unwrap();
        let events = &blocks.children[0];
        assert_eq!(block_of(events).as_deref(), Some("events"));
        // The column child resolves to its parent table.
        assert_eq!(block_of(&events.children[0]).as_deref(), Some("events"));
        // The blocks group itself has no block handle.
        assert_eq!(block_of(blocks), None);
    }

    #[test]
    fn missing_selection_is_a_friendly_unavailable() {
        let m = tessera_core::Manifest::new("p", "n", "d", "2024-01-01T00:00:00Z");
        let tree = hierarchy(&m, &[]);
        // The root product node has no block handle → a guiding message, not an error/panic.
        let v = DataView::load(Path::new("/nonexistent.tsra"), &tree.root);
        assert!(matches!(v, DataView::Unavailable(msg) if msg.contains("Select a block")));
    }
}
