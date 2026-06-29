//! `tessera tree` / `ls` / `read` — navigate + extract a `.tsra` as a self-describing hierarchy.
//!
//! A `.tsra` is a STORED zip with a *defined* structure (manifest spine + shape-dispatched blocks),
//! so it browses like a zarr group: the product is the root, metadata fields are attributes, and each
//! block is an array or a (possibly multi-block) table whose columns are the leaves. `tree` renders
//! the whole hierarchy, `ls` lists one node's children, and `read` extracts table data — the latter
//! over the **logical** table view (`tessera_io::LogicalTableView`), so a column read spans every
//! `events_NNNN` block transparently (the cross-block query, on the command line).
//!
//! Output goes to a caller-supplied `Write` (not `println!`) so the commands are unit-testable and
//! the binary's `main` owns the actual stdout/stderr — `main.rs` is the CLI entrypoint that may print.

use std::io::Write;
use std::path::Path;

use serde_json::Value;
use tessera_core::block::BlockKind;
use tessera_core::{Result, SchemaRegistry};
use tessera_io::{ColumnData, Reader};

/// Row-delimited output formats for [`read`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Comma-separated, one header row + one row per record.
    Csv,
    /// Tab-separated (same shape as [`Format::Csv`]).
    Tsv,
    /// Newline-delimited JSON — one `{column: value, …}` object per record.
    Ndjson,
}

impl Format {
    /// Parse the `--format` flag value; defaults are handled by the caller.
    pub fn parse(s: &str) -> Result<Format> {
        match s {
            "csv" => Ok(Format::Csv),
            "tsv" => Ok(Format::Tsv),
            "ndjson" | "jsonl" => Ok(Format::Ndjson),
            other => Err(tessera_core::Error::Invalid(format!(
                "unknown --format '{other}' (expected csv | tsv | ndjson)"
            ))),
        }
    }

    fn sep(self) -> char {
        match self {
            Format::Tsv => '\t',
            _ => ',',
        }
    }
}

/// Shorten a `blake3:<hex>` digest to a glanceable prefix for tree/inspect rendering.
fn short_digest(d: Option<&str>) -> String {
    match d {
        Some(s) => {
            // Keep the algorithm tag + the first 12 hex nibbles: `blake3:1a2b3c4d5e6f…`.
            if let Some((alg, hex)) = s.split_once(':') {
                let head: String = hex.chars().take(12).collect();
                if hex.len() > 12 {
                    format!("{alg}:{head}…")
                } else {
                    format!("{alg}:{head}")
                }
            } else {
                s.to_string()
            }
        }
        None => "-".to_string(),
    }
}

/// Group-of-three thousands separators for human row counts (`4194304` → `4,194,304`).
fn thousands(n: u64) -> String {
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

/// Render `spec["shape"]` (a JSON array of ints) as `[128, 512, 512]`.
fn shape_str(spec: &Value) -> String {
    match spec.get("shape").and_then(Value::as_array) {
        Some(dims) => {
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
        None => "[?]".to_string(),
    }
}

/// One-line block summary (`array int16 [..] pcodec` / `table 6 cols × 4,194,304 rows`).
fn block_headline(kind: &BlockKind, spec: &Value) -> String {
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
    }
}

/// Child lines for a block: column `name dtype` rows for tables, spec detail for arrays.
fn block_children(kind: &BlockKind, spec: &Value) -> Vec<String> {
    match kind {
        BlockKind::Array => {
            let chunks = spec
                .get("chunks")
                .and_then(Value::as_array)
                .map(|c| {
                    let parts: Vec<String> = c
                        .iter()
                        .map(|d| {
                            d.as_u64()
                                .map(|u| u.to_string())
                                .unwrap_or_else(|| "?".into())
                        })
                        .collect();
                    format!("[{}]", parts.join(", "))
                })
                .unwrap_or_else(|| "[?]".into());
            vec![format!("chunks {chunks}")]
        }
        BlockKind::Table => spec
            .get("columns")
            .and_then(Value::as_array)
            .map(|cols| {
                cols.iter()
                    .map(|c| {
                        let n = c.get("name").and_then(Value::as_str).unwrap_or("?");
                        let d = c.get("dtype").and_then(Value::as_str).unwrap_or("?");
                        format!("{n:<10} {d}")
                    })
                    .collect()
            })
            .unwrap_or_default(),
        BlockKind::ChunkIndex => Vec::new(),
    }
}

/// `product` + schema-validity + seal + signature badges for the tree root / inspect header.
fn status_line(file: &Path, m: &tessera_core::Manifest) -> String {
    let reg = SchemaRegistry::builtin();
    let schema = match reg.get(&m.product) {
        Some(_) => match reg.validate(m) {
            Ok(()) => format!("schema={}✓", m.product),
            Err(_) => format!("schema={}✗", m.product),
        },
        None => format!("schema={}(open-world)", m.product),
    };
    let sealed = if m.manifest_hash.is_some() {
        "sealed"
    } else {
        "unsealed"
    };
    let signed = if tessera_io::sign::sidecar_path(file).exists() {
        " · signed"
    } else {
        ""
    };
    format!("product={} · {schema} · {sealed}{signed}", m.product)
}

/// `tessera tree FILE` — the whole hierarchy: root status, `meta` fields, every block (with its
/// columns / array spec), and `sources`, drawn with box characters.
pub fn tree(file: &Path, out: &mut dyn Write) -> Result<()> {
    let r = Reader::open(file)?;
    let m = r.manifest();
    let name = file
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<tsra>");
    writeln!(out, "{name}  ·  {}", status_line(file, m)).map_err(tessera_core::Error::from)?;

    // Build the node list: (header, children). meta first, then blocks, then sources.
    let mut nodes: Vec<(String, Vec<String>)> = Vec::new();
    if !m.metadata.is_empty() {
        let kids = m
            .metadata
            .iter()
            .map(|(k, v)| format!("{k} = {}", compact_value(v)))
            .collect();
        nodes.push(("meta".to_string(), kids));
    }
    for b in &m.blocks {
        let header = format!(
            "{:<18} {}   {}",
            b.name,
            block_headline(&b.kind, &b.spec),
            short_digest(b.digest.as_deref())
        );
        nodes.push((header, block_children(&b.kind, &b.spec)));
    }
    if !m.sources.is_empty() {
        let kids = m
            .sources
            .iter()
            .map(|s| format!("{} <- {}", s.role, s.reference))
            .collect();
        nodes.push(("sources".to_string(), kids));
    }

    let last_node = nodes.len().saturating_sub(1);
    for (i, (header, kids)) in nodes.iter().enumerate() {
        let is_last = i == last_node;
        let (branch, cont) = if is_last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        writeln!(out, "{branch}{header}").map_err(tessera_core::Error::from)?;
        let last_kid = kids.len().saturating_sub(1);
        for (j, kid) in kids.iter().enumerate() {
            let kbranch = if j == last_kid {
                "└── "
            } else {
                "├── "
            };
            writeln!(out, "{cont}{kbranch}{kid}").map_err(tessera_core::Error::from)?;
        }
    }
    Ok(())
}

/// Compact one-line render of a metadata JSON value (truncated if long).
fn compact_value(v: &Value) -> String {
    let s = match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.chars().count() > 60 {
        let head: String = s.chars().take(57).collect();
        format!("{head}…")
    } else {
        s
    }
}

/// `tessera ls FILE [PATH]` — list one node's children. No PATH lists the top level (`meta`, each
/// block, `sources`); `PATH=meta` lists metadata fields; `PATH=<block>` lists a table's columns or
/// an array's spec; `PATH=sources` lists provenance edges.
pub fn ls(file: &Path, path: Option<&str>, out: &mut dyn Write) -> Result<()> {
    let r = Reader::open(file)?;
    let m = r.manifest();
    match path {
        None => {
            if !m.metadata.is_empty() {
                writeln!(out, "meta/  ({} fields)", m.metadata.len())
                    .map_err(tessera_core::Error::from)?;
            }
            for b in &m.blocks {
                writeln!(out, "{:<18} {:?}", b.name, b.kind).map_err(tessera_core::Error::from)?;
            }
            if !m.sources.is_empty() {
                writeln!(out, "sources/  ({} edges)", m.sources.len())
                    .map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some("meta") | Some("meta/") => {
            for (k, v) in &m.metadata {
                writeln!(out, "{k} = {}", compact_value(v)).map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some("sources") | Some("sources/") => {
            for s in &m.sources {
                writeln!(out, "{} <- {}", s.role, s.reference)
                    .map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some(node) => {
            let b = m.blocks.iter().find(|b| b.name == node).ok_or_else(|| {
                tessera_core::Error::Invalid(format!(
                    "no node '{node}' in {} (try `tessera ls {}` for the top level)",
                    file.display(),
                    file.display()
                ))
            })?;
            writeln!(out, "{}   {}", node, block_headline(&b.kind, &b.spec))
                .map_err(tessera_core::Error::from)?;
            for kid in block_children(&b.kind, &b.spec) {
                writeln!(out, "  {kid}").map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
    }
}

/// Options for [`read`].
pub struct ReadOpts<'a> {
    /// The `.tsra` to read.
    pub file: &'a Path,
    /// The table block (or multi-block prefix like `events`) to extract.
    pub block: &'a str,
    /// Columns to project (empty = all columns, in schema order).
    pub columns: Vec<String>,
    /// Global row range `[lo, hi)` (None = from row 0).
    pub rows: Option<(u64, u64)>,
    /// Emit every row (overrides `limit`).
    pub all: bool,
    /// Default row cap when neither `rows` nor `all` is given.
    pub limit: u64,
    /// Output format.
    pub format: Format,
}

/// Summary of what [`read`] emitted, so the caller (`main`) can print a truncation note to stderr.
pub struct ReadResult {
    /// Rows actually written.
    pub shown: u64,
    /// Total rows in the (logical) table.
    pub total: u64,
    /// True if `shown < total` because the default `limit` capped the output.
    pub truncated: bool,
}

/// `tessera read FILE BLOCK [--column C]… [--rows A:B | --all]` — extract table data over the
/// **logical** view, so a read of `events` spans every `events_NNNN` block (cross-block query).
/// Columns are projected (only the requested columns' segments are decoded per block).
pub fn read(opts: ReadOpts, out: &mut dyn Write) -> Result<ReadResult> {
    let mut r = Reader::open(opts.file)?;
    let view = r.logical_table(opts.block)?;
    let total = view.row_count();

    // Resolve the column projection against the table schema (clear error on a typo).
    let all_names: Vec<String> = view.columns().iter().map(|c| c.name.clone()).collect();
    let selected: Vec<String> = if opts.columns.is_empty() {
        all_names.clone()
    } else {
        for c in &opts.columns {
            if !all_names.iter().any(|n| n == c) {
                return Err(tessera_core::Error::Invalid(format!(
                    "no column '{c}' in '{}' (columns: {})",
                    opts.block,
                    all_names.join(", ")
                )));
            }
        }
        opts.columns.clone()
    };

    // Resolve the row window: explicit range, or all, or the default cap.
    let (lo, hi) = match opts.rows {
        Some((a, b)) => (a.min(total), b.min(total)),
        None if opts.all => (0, total),
        None => (0, opts.limit.min(total)),
    };
    let truncated = opts.rows.is_none() && !opts.all && hi < total;
    let nrows = hi.saturating_sub(lo);

    // Decode each selected column (projected), slice to the window, stringify to JSON cells.
    let lo_us = usize::try_from(lo).map_err(|e| tessera_core::Error::Invalid(e.to_string()))?;
    let hi_us = usize::try_from(hi).map_err(|e| tessera_core::Error::Invalid(e.to_string()))?;
    let mut cells: Vec<Vec<Value>> = Vec::with_capacity(selected.len());
    for name in &selected {
        let col = view.column(&mut r, name)?;
        cells.push(col_to_values(&col.slice(lo_us, hi_us)));
    }

    // Header (csv/tsv only).
    if matches!(opts.format, Format::Csv | Format::Tsv) {
        let sep = opts.format.sep();
        let header: Vec<&str> = selected.iter().map(String::as_str).collect();
        writeln!(out, "{}", header.join(&sep.to_string())).map_err(tessera_core::Error::from)?;
    }

    let n = usize::try_from(nrows).map_err(|e| tessera_core::Error::Invalid(e.to_string()))?;
    for row in 0..n {
        match opts.format {
            Format::Csv | Format::Tsv => {
                let sep = opts.format.sep();
                let line: Vec<String> = cells
                    .iter()
                    .map(|c| csv_cell(c.get(row).unwrap_or(&Value::Null)))
                    .collect();
                writeln!(out, "{}", line.join(&sep.to_string()))
                    .map_err(tessera_core::Error::from)?;
            }
            Format::Ndjson => {
                let mut obj = serde_json::Map::with_capacity(selected.len());
                for (i, name) in selected.iter().enumerate() {
                    let v = cells
                        .get(i)
                        .and_then(|c| c.get(row))
                        .cloned()
                        .unwrap_or(Value::Null);
                    obj.insert(name.clone(), v);
                }
                writeln!(out, "{}", Value::Object(obj)).map_err(tessera_core::Error::from)?;
            }
        }
    }

    Ok(ReadResult {
        shown: nrows,
        total,
        truncated,
    })
}

/// Render a JSON cell for CSV/TSV: numbers bare, JSON-null (e.g. NaN/±inf floats) as `nan`.
fn csv_cell(v: &Value) -> String {
    match v {
        Value::Null => "nan".to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Convert a (sliced) numeric column to per-row JSON values. Floats render via their **native**
/// shortest round-trip `Display` (so an `f32` shows `0.01`, not its widened-`f64` expansion);
/// non-finite floats (NaN/±inf) have no JSON encoding → null (CSV shows `nan`, ndjson `null`).
fn col_to_values(col: &ColumnData) -> Vec<Value> {
    fn floats<T: std::fmt::Display + Copy>(v: &[T]) -> Vec<Value> {
        v.iter()
            .map(|x| {
                x.to_string()
                    .parse::<serde_json::Number>()
                    .map_or(Value::Null, Value::Number)
            })
            .collect()
    }
    match col {
        ColumnData::I8(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::I16(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::I32(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::I64(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U8(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U16(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U32(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::U64(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::F32(v) => floats(v),
        ColumnData::F64(v) => floats(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder;
    use tessera_io::{pack, table::TableData, ColumnData};

    fn sample(path: &Path) {
        let spec = TableSpec {
            columns: vec![
                Column {
                    name: "ms".into(),
                    dtype: "u4".into(),
                    codec: None,
                },
                Column {
                    name: "en".into(),
                    dtype: "f4".into(),
                    codec: None,
                },
            ],
            rows: 4,
            row_index: None,
        };
        let data: TableData = vec![
            ("ms".into(), ColumnData::U32(vec![10, 20, 30, 40])),
            ("en".into(), ColumnData::F32(vec![0.5, 1.5, 2.5, 3.5])),
        ];
        // Build the block + payload via the engine helper so the recorded digest matches the
        // packed Vortex bytes exactly (a hand-rolled payload would fail the seal's integrity check).
        let (block_ref, payload) = tessera_io::table::table_block("events", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("listmode", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block_ref);
        b.with_field("modality", serde_json::json!("PT"));
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], path).unwrap();
    }

    #[test]
    fn tree_renders_root_meta_block_columns() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.tsra");
        sample(&p);
        let mut buf = Vec::new();
        tree(&p, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("product=listmode"));
        assert!(s.contains("schema=listmode")); // known schema; ✓/✗ depends on field completeness
        assert!(s.contains("meta"));
        assert!(s.contains("modality"));
        assert!(s.contains("events"));
        assert!(s.contains("ms")); // a column leaf
    }

    #[test]
    fn ls_top_then_block_columns() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.tsra");
        sample(&p);
        let mut top = Vec::new();
        ls(&p, None, &mut top).unwrap();
        assert!(String::from_utf8(top).unwrap().contains("events"));
        let mut cols = Vec::new();
        ls(&p, Some("events"), &mut cols).unwrap();
        let s = String::from_utf8(cols).unwrap();
        assert!(s.contains("ms") && s.contains("en"));
    }

    #[test]
    fn read_csv_projects_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.tsra");
        sample(&p);
        let mut buf = Vec::new();
        let res = read(
            ReadOpts {
                file: &p,
                block: "events",
                columns: vec!["ms".into()],
                rows: None,
                all: false,
                limit: 2,
                format: Format::Csv,
            },
            &mut buf,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        // header + 2 data rows; en column omitted by the projection
        assert_eq!(s.lines().next(), Some("ms"));
        assert_eq!(s.lines().count(), 3);
        assert!(s.contains("10") && s.contains("20") && !s.contains("30"));
        assert!(res.truncated && res.shown == 2 && res.total == 4);
    }

    #[test]
    fn read_ndjson_all_rows() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.tsra");
        sample(&p);
        let mut buf = Vec::new();
        read(
            ReadOpts {
                file: &p,
                block: "events",
                columns: vec![],
                rows: None,
                all: true,
                limit: 2,
                format: Format::Ndjson,
            },
            &mut buf,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.lines().count(), 4);
        assert!(s.contains("\"ms\":10"));
        assert!(s.contains("\"en\":0.5"));
    }
}
