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
use tessera_io::array::ArrayData;
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

/// Short form of a single `blake3:<hex>` hash for inline display (`blake3:1a2b3c4d5e6f…`).
pub(crate) fn short_hash(h: &str) -> String {
    short_digest(Some(h))
}

/// Parse the embedded schema JSON (`Manifest.schema`) into a typed schema, or `None` if absent/bad.
fn embedded_schema(v: &Value) -> Option<tessera_core::ProductSchema> {
    tessera_core::ProductSchema::from_value(v).ok()
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

/// Human-readable byte size (`3.0 GiB`, `512 KiB`) for blob block display.
fn human_bytes(n: u64) -> String {
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
        BlockKind::Blob => spec
            .get("filename")
            .and_then(Value::as_str)
            .map(|f| vec![format!("file   {f}")])
            .unwrap_or_default(),
    }
}

/// `product` + schema-validity + seal + signature badges for the tree root / inspect header.
/// Validation is against the **embedded** schema when the file carries one (self-describing), else
/// the built-in registry (legacy / open-world) — see [`tessera_core::validate_manifest`].
fn status_line(file: &Path, m: &tessera_core::Manifest) -> String {
    let known = m.schema.is_some() || SchemaRegistry::builtin().get(&m.product).is_some();
    let schema = if known {
        match tessera_core::validate_manifest(m) {
            Ok(()) => format!("schema={}✓", m.product),
            Err(_) => format!("schema={}✗", m.product),
        }
    } else {
        format!("schema={}(open-world)", m.product)
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
pub fn tree(file: &Path, full: bool, out: &mut dyn Write) -> Result<()> {
    let r = Reader::open(file)?;
    let m = r.manifest();
    let name = file
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<tsra>");
    writeln!(out, "{name}  ·  {}", status_line(file, m)).map_err(tessera_core::Error::from)?;

    // Build the node list: (header, children). meta · schema · blocks · sources · extra.
    let mut nodes: Vec<(String, Vec<String>)> = Vec::new();
    if !m.metadata.is_empty() {
        let kids = m
            .metadata
            .iter()
            .map(|(k, v)| format!("{k} = {}", compact_value(v, full)))
            .collect();
        nodes.push(("meta".to_string(), kids));
    }
    // The embedded, self-describing product schema (its declared fields as leaves).
    if let Some(s) = m.schema.as_ref().and_then(embedded_schema) {
        let kids = s
            .fields
            .iter()
            .map(|f| {
                let tier = if f.required {
                    "required"
                } else if f.recommended {
                    "recommended"
                } else {
                    "optional"
                };
                format!(
                    "{:<22} {tier} · {}",
                    f.id,
                    format!("{:?}", f.sensitivity).to_lowercase()
                )
            })
            .collect();
        nodes.push((format!("schema  ({} v{})", s.product, s.version), kids));
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
            .map(|s| format!("{} <- {}", s.role, compact_reference(&s.reference, full)))
            .collect();
        nodes.push(("sources".to_string(), kids));
    }
    // The extension namespace (fd5 `extra/`) — the full DICOM header + other vendor/provenance blobs.
    if !m.extra.is_empty() {
        let kids = m
            .extra
            .iter()
            .map(|(k, v)| {
                let kind = match v {
                    Value::Object(o) => format!("object, {} keys", o.len()),
                    Value::Array(a) => format!("array, {} items", a.len()),
                    Value::String(_) => "string".into(),
                    other => other.to_string(),
                };
                format!("{k}  ({kind})")
            })
            .collect();
        nodes.push(("extra".to_string(), kids));
    }
    // Adjacent sidecar files (outside the seal): the detached signature (ADR-0037), and — when
    // present — the field-encryption envelope (ADR-0041). The seal itself is in the manifest, not
    // a sidecar. Shown so `tree` reflects the whole on-disk product, not just the container.
    let sidecars: Vec<String> = [
        ("signature", tessera_io::sign::sidecar_path(file)),
        ("field-encryption", file.with_extension("tsra.fcrypt.json")),
    ]
    .into_iter()
    .filter(|(_, p)| p.exists())
    .map(|(kind, p)| {
        format!(
            "{kind}: {}",
            p.file_name().and_then(|s| s.to_str()).unwrap_or("?")
        )
    })
    .collect();
    if !sidecars.is_empty() {
        nodes.push(("sidecars".to_string(), sidecars));
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

/// Compact one-line render of a metadata JSON value. Truncated at 60 chars unless `full`.
fn compact_value(v: &Value, full: bool) -> String {
    let s = match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if !full && s.chars().count() > 60 {
        let head: String = s.chars().take(57).collect();
        format!("{head}…")
    } else {
        s
    }
}

/// Middle-elide a string to `max` chars, keeping the head **and** the (informative) tail — for a
/// filesystem path that means the filename survives. Returns as-is if already within `max`.
fn elide(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1); // room for the ellipsis
    let head = keep / 2;
    let tail = keep - head;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    format!("{h}…{t}")
}

/// Longest common **directory** prefix (path-component-wise) of a set of paths — the shared parent
/// that lets `ls sources` print a group header once and relative filenames under it.
fn common_dir<'a>(paths: &[&'a str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    fn dir_of(p: &str) -> &str {
        p.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
    }
    let mut prefix: Vec<&'a str> = dir_of(paths[0]).split('/').collect();
    for p in &paths[1..] {
        let comps: Vec<&str> = dir_of(p).split('/').collect();
        let common = prefix
            .iter()
            .zip(comps.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(common);
    }
    prefix.join("/")
}

/// Compact one-line render of a provenance-edge reference. A DICOM-series `ingested_from` holds a
/// **comma-joined list of every slice path** — rendered raw it floods the terminal with hundreds of
/// KB. Collapse a list to `<first path> (+N more)` and middle-elide a long single path so its
/// filename tail stays visible. `--full` bypasses this and prints the reference verbatim.
pub(crate) fn compact_reference(reference: &str, full: bool) -> String {
    if full {
        return reference.to_string();
    }
    if let Some((first, rest)) = reference.split_once(',') {
        let more = rest.split(',').filter(|s| !s.trim().is_empty()).count();
        return format!("{} (+{more} more)", elide(first, 72));
    }
    elide(reference, 96)
}

/// The `ls sources` render of one provenance edge, as output lines. A single-file edge is one line
/// (`role <- path`); a multi-file edge (a DICOM series) becomes a **group** — a `role <- N files in
/// <common-dir>/` header, then one relative filename per line. Default caps the body at 8 entries
/// with a `… (+N more)` footer; `full` lists every file. Pure (returns lines) so it is unit-testable.
fn source_lines(role: &str, reference: &str, digest: Option<&str>, full: bool) -> Vec<String> {
    // The `content_hash` on the edge — the integrity link (source merkle root for `ingested_from`,
    // parent `manifest_hash` / spec_hash for derived/spec edges) — shown after the header.
    let integ = digest
        .map(|h| format!("  [{}]", short_hash(h)))
        .unwrap_or_default();
    let items: Vec<&str> = reference
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .collect();
    if items.len() <= 1 {
        let one = if full {
            reference.to_string()
        } else {
            elide(reference, 96)
        };
        return vec![format!("{role} <- {one}{integ}")];
    }
    let dir = common_dir(&items);
    let where_ = if dir.is_empty() {
        String::new()
    } else {
        format!(" in {dir}/")
    };
    let mut lines = vec![format!("{role} <- {} files{where_}{integ}", items.len())];
    let show = if full {
        items.len()
    } else {
        items.len().min(8)
    };
    for it in &items[..show] {
        let rel = it.strip_prefix(&dir).unwrap_or(it).trim_start_matches('/');
        lines.push(format!("    {rel}"));
    }
    if items.len() > show {
        lines.push(format!(
            "    … (+{} more, --full to list all)",
            items.len() - show
        ));
    }
    lines
}

/// `tessera ls FILE [PATH]` — list one node's children. No PATH lists the top level (`meta`, each
/// block, `sources`); `PATH=meta` lists metadata fields; `PATH=<block>` lists a table's columns or
/// an array's spec; `PATH=sources` lists provenance edges.
pub fn ls(file: &Path, path: Option<&str>, full: bool, out: &mut dyn Write) -> Result<()> {
    let r = Reader::open(file)?;
    let m = r.manifest();
    match path {
        None => {
            if !m.metadata.is_empty() {
                writeln!(out, "meta/  ({} fields)", m.metadata.len())
                    .map_err(tessera_core::Error::from)?;
            }
            // The embedded product schema (self-describing) — navigable so `ls FILE schema` shows the
            // declared field roster the file carries its own contract for.
            if let Some(s) = m.schema.as_ref().and_then(embedded_schema) {
                writeln!(
                    out,
                    "schema/  ({} v{}, {} fields)",
                    s.product,
                    s.version,
                    s.fields.len()
                )
                .map_err(tessera_core::Error::from)?;
            }
            for b in &m.blocks {
                writeln!(out, "{:<18} {:?}", b.name, b.kind).map_err(tessera_core::Error::from)?;
            }
            if !m.sources.is_empty() {
                writeln!(out, "sources/  ({} edges)", m.sources.len())
                    .map_err(tessera_core::Error::from)?;
            }
            // The extension namespace (fd5 `extra/`) — vendor/provenance blobs like the full DICOM
            // header (`dicom_header`) live here; `ls FILE extra/<key>` dumps one.
            if !m.extra.is_empty() {
                writeln!(out, "extra/  ({} keys)", m.extra.len())
                    .map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some("meta") | Some("meta/") => {
            for (k, v) in &m.metadata {
                writeln!(out, "{k} = {}", compact_value(v, full))
                    .map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some("schema") | Some("schema/") => {
            match m.schema.as_ref().and_then(embedded_schema) {
                Some(s) => {
                    writeln!(out, "{} v{} — {}", s.product, s.version, s.description)
                        .map_err(tessera_core::Error::from)?;
                    for f in &s.fields {
                        let tier = if f.required {
                            "required"
                        } else if f.recommended {
                            "recommended"
                        } else {
                            "optional"
                        };
                        let sens = format!("{:?}", f.sensitivity).to_lowercase();
                        writeln!(out, "  {:<22} {tier:<12} {sens}", f.id)
                            .map_err(tessera_core::Error::from)?;
                    }
                }
                None => writeln!(
                    out,
                    "no embedded schema (file predates self-describing schemas; `tsra schema` uses the registry)"
                )
                .map_err(tessera_core::Error::from)?,
            }
            Ok(())
        }
        Some(p) if p == "extra" || p == "extra/" => {
            for (k, v) in &m.extra {
                let kind = match v {
                    Value::Object(o) => format!("object, {} keys", o.len()),
                    Value::Array(a) => format!("array, {} items", a.len()),
                    Value::String(_) => "string".into(),
                    other => other.to_string(),
                };
                writeln!(out, "{k}  ({kind})").map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some(p) if p.starts_with("extra/") => {
            let key = &p["extra/".len()..];
            match m.extra.get(key) {
                Some(v) => writeln!(out, "{}", serde_json::to_string_pretty(v)?)
                    .map_err(tessera_core::Error::from)?,
                None => {
                    return Err(tessera_core::Error::Invalid(format!(
                        "no extra key '{key}' (keys: {})",
                        m.extra.keys().cloned().collect::<Vec<_>>().join(", ")
                    )))
                }
            }
            Ok(())
        }
        Some("sources") | Some("sources/") => {
            // `ls sources` is the drill-down: a multi-file edge (a DICOM series' `ingested_from`
            // holds a comma-joined path list) is exploded and **grouped by common directory** so it
            // reads as a real listing — count + shared dir header, then relative filenames.
            for s in &m.sources {
                for line in source_lines(&s.role, &s.reference, s.content_hash.as_deref(), full) {
                    writeln!(out, "{line}").map_err(tessera_core::Error::from)?;
                }
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

/// A row selection for [`read`], resolved against the table's row count **at read time** — so
/// negative (from-the-end) and open (`N:`, `:N`, `:`) bounds work without the CLI knowing the total
/// up front. Half-open throughout (`[lo, hi)`), Python-slice semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowSpec {
    /// `--rows A:B`: each bound optional (open = start/end) and negative = counted from the end.
    Range { lo: Option<i64>, hi: Option<i64> },
    /// `--head N`: the first N rows.
    Head(u64),
    /// `--tail N`: the last N rows.
    Tail(u64),
    /// `--at I`: exactly the one row at index I (negative = from the end).
    At(i64),
}

impl RowSpec {
    /// Parse a `--rows` value: `A:B` with optional/negative bounds (`91500:`, `:100`, `-10:-1`, `:`).
    pub fn parse_range(s: &str) -> Result<RowSpec> {
        let (a, b) = s.split_once(':').ok_or_else(|| {
            tessera_core::Error::Invalid(format!(
                "--rows expects A:B (half-open); for a single row use --at, got '{s}'"
            ))
        })?;
        let bound = |x: &str, side: &str| -> Result<Option<i64>> {
            let x = x.trim();
            if x.is_empty() {
                return Ok(None);
            }
            x.parse::<i64>().map(Some).map_err(|_| {
                tessera_core::Error::Invalid(format!("--rows: bad {side} bound '{x}'"))
            })
        };
        Ok(RowSpec::Range {
            lo: bound(a, "lower")?,
            hi: bound(b, "upper")?,
        })
    }

    /// Resolve to a concrete half-open `[lo, hi)` clamped to `[0, total]`. Negative bounds count from
    /// the end; an inverted range (`lo > hi`) yields an empty window (Python-slice behaviour).
    pub fn resolve(self, total: u64) -> (u64, u64) {
        let t = total as i64;
        let idx = |v: i64| -> u64 { (if v < 0 { t + v } else { v }).clamp(0, t) as u64 };
        match self {
            RowSpec::Range { lo, hi } => {
                let l = lo.map(idx).unwrap_or(0);
                let h = hi.map(idx).unwrap_or(total);
                (l, h.max(l))
            }
            RowSpec::Head(n) => (0, n.min(total)),
            RowSpec::Tail(n) => (total.saturating_sub(n), total),
            RowSpec::At(i) => {
                let a = idx(i);
                (a, (a + 1).min(total))
            }
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
    /// Explicit row selection (`--rows`/`--head`/`--tail`/`--at`), resolved against the row count at
    /// read time. `None` = fall back to `limit`.
    pub rows: Option<RowSpec>,
    /// Emit every row (overrides `limit`).
    pub all: bool,
    /// Default row cap when neither `rows` nor `all` is given.
    pub limit: u64,
    /// Output format.
    pub format: Format,
}

/// Summary of what [`read`] emitted, so the caller (`main`) can print a truncation note to stderr.
#[derive(Debug)]
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
    // `read` is table-only. If the target names an **array** block (a volume/μ-map), fail with a
    // clear pointer instead of the opaque "missing field columns" from the table decoder (#253).
    if let Some(b) = r.manifest().blocks.iter().find(|b| b.name == opts.block) {
        if b.kind == BlockKind::Array {
            return Err(tessera_core::Error::Invalid(format!(
                "'{}' is an array block ({}), not a table — `read` is for tables. Use \
                 `tsra ls {} {}` for its spec or `tsra extract` for raw bytes (array slicing/stats: #253).",
                opts.block,
                block_headline(&b.kind, &b.spec),
                opts.file.display(),
                opts.block,
            )));
        }
    }
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

    // Resolve the row window: explicit selection (resolved vs the row count), or all, or the cap.
    let (lo, hi) = match opts.rows {
        Some(spec) => spec.resolve(total),
        None if opts.all => (0, total),
        None => (0, opts.limit.min(total)),
    };
    // Only the default-cap path is a silent truncation worth warning about.
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

/// Load an **array** block: open the file, confirm the named block is an array (not a table), parse
/// its `ArraySpec`, and read the raw (encoded) payload. Shared by [`stats`] and [`slice`].
fn open_array(
    file: &Path,
    block: &str,
) -> Result<(tessera_core::block::array::ArraySpec, Vec<u8>)> {
    let mut r = Reader::open(file)?;
    let bref = r
        .manifest()
        .blocks
        .iter()
        .find(|b| b.name == block)
        .ok_or_else(|| tessera_core::Error::Invalid(format!("no block '{block}' in this .tsra")))?;
    if bref.kind != BlockKind::Array {
        return Err(tessera_core::Error::Invalid(format!(
            "block '{block}' is a {:?}, not an array — `stats`/`slice` are for array blocks",
            bref.kind
        )));
    }
    let spec: tessera_core::block::array::ArraySpec = serde_json::from_value(bref.spec.clone())
        .map_err(|e| tessera_core::Error::Invalid(format!("bad array spec for '{block}': {e}")))?;
    let blob = r.read_block(block)?;
    Ok((spec, blob))
}

/// Min / max / mean / std over an [`ArrayData`], computed in `f64` (one pass). Empty → all zero.
fn array_stats(d: &ArrayData) -> (f64, f64, f64, f64, usize) {
    macro_rules! reduce {
        ($v:expr) => {{
            let n = $v.len();
            if n == 0 {
                (0.0, 0.0, 0.0, 0.0, 0)
            } else {
                let mut mn = f64::INFINITY;
                let mut mx = f64::NEG_INFINITY;
                let mut sum = 0.0f64;
                let mut sumsq = 0.0f64;
                for &x in $v.iter() {
                    let x = x as f64;
                    mn = mn.min(x);
                    mx = mx.max(x);
                    sum += x;
                    sumsq += x * x;
                }
                let mean = sum / n as f64;
                let var = (sumsq / n as f64) - mean * mean;
                (mn, mx, mean, var.max(0.0).sqrt(), n)
            }
        }};
    }
    match d {
        ArrayData::I16(v) => reduce!(v),
        ArrayData::I32(v) => reduce!(v),
        ArrayData::I64(v) => reduce!(v),
        ArrayData::U16(v) => reduce!(v),
        ArrayData::U32(v) => reduce!(v),
        ArrayData::U64(v) => reduce!(v),
        ArrayData::F32(v) => reduce!(v),
        ArrayData::F64(v) => reduce!(v),
    }
}

/// `tessera stats FILE BLOCK` — a numeric overview of an **array** block: shape · dtype · chunks ·
/// codec · value range (min/max/mean/std, raw and — when a rescale is present — physical) · unit ·
/// spatial referencing. Decodes the block once; the "general looking at it" for a volume.
pub fn stats(file: &Path, block: &str, out: &mut dyn Write) -> Result<()> {
    let (spec, blob) = open_array(file, block)?;
    let data = tessera_io::array::decode(&spec, &blob)?;
    let (mn, mx, mean, std, n) = array_stats(&data);

    let shape: Vec<String> = spec.shape.iter().map(u64::to_string).collect();
    let axes = if spec.axes.is_empty() {
        String::new()
    } else {
        format!(" ({})", spec.axes.join(","))
    };
    writeln!(out, "block     {block}").map_err(tessera_core::Error::from)?;
    writeln!(out, "shape     [{}]{axes}", shape.join(", ")).map_err(tessera_core::Error::from)?;
    writeln!(out, "dtype     {}   codec {}", spec.dtype, spec.codec)
        .map_err(tessera_core::Error::from)?;
    let chunks: Vec<String> = spec.chunks.iter().map(u64::to_string).collect();
    writeln!(
        out,
        "chunks    [{}]   voxels {}",
        chunks.join(", "),
        thousands(n as u64)
    )
    .map_err(tessera_core::Error::from)?;
    writeln!(
        out,
        "raw       min {mn}  max {mx}  mean {mean:.3}  std {std:.3}"
    )
    .map_err(tessera_core::Error::from)?;
    // Physical units (CT→HU, PET→Bq/mL) when the array carries a rescale.
    if let (Some(sl), Some(ic)) = (spec.rescale_slope, spec.rescale_intercept) {
        let unit = spec.unit.as_deref().unwrap_or("");
        writeln!(
            out,
            "physical  min {}  max {}  ({}·raw + {}) {unit}",
            sl * mn + ic,
            sl * mx + ic,
            sl,
            ic
        )
        .map_err(tessera_core::Error::from)?;
    }
    match &spec.world_frame {
        Some(wf) => writeln!(
            out,
            "world     {} affine present ({})",
            wf.convention, wf.unit
        )
        .map_err(tessera_core::Error::from)?,
        None => writeln!(
            out,
            "world     index space (no affine — use --index, not --world)"
        )
        .map_err(tessera_core::Error::from)?,
    }
    Ok(())
}

/// Parse a numpy-style index like `445,:,:` or `400:500,:,256` against `shape` into per-axis
/// `(start, len)` for [`tessera_io::array::decode_subset`]. Each token is `N` (one index, negative
/// from end), `:` (whole axis), or `A:B` (half-open, optional/negative bounds).
fn parse_index(index: &str, shape: &[u64]) -> Result<(Vec<u64>, Vec<u64>)> {
    let toks: Vec<&str> = index.split(',').map(str::trim).collect();
    if toks.len() != shape.len() {
        return Err(tessera_core::Error::Invalid(format!(
            "--index has {} axes but the array has {} (shape [{}])",
            toks.len(),
            shape.len(),
            shape
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let mut start = Vec::with_capacity(shape.len());
    let mut len = Vec::with_capacity(shape.len());
    for (tok, &dim) in toks.iter().zip(shape.iter()) {
        let d = dim as i64;
        let resolve = |v: i64| -> u64 { (if v < 0 { d + v } else { v }).clamp(0, d) as u64 };
        if *tok == ":" {
            start.push(0);
            len.push(dim);
        } else if let Some((a, b)) = tok.split_once(':') {
            let lo = if a.trim().is_empty() {
                0
            } else {
                resolve(a.trim().parse().map_err(|_| {
                    tessera_core::Error::Invalid(format!("--index: bad range start '{a}'"))
                })?)
            };
            let hi = if b.trim().is_empty() {
                dim
            } else {
                resolve(b.trim().parse().map_err(|_| {
                    tessera_core::Error::Invalid(format!("--index: bad range end '{b}'"))
                })?)
            };
            start.push(lo);
            len.push(hi.saturating_sub(lo));
        } else {
            let i = resolve(tok.parse().map_err(|_| {
                tessera_core::Error::Invalid(format!("--index: bad index '{tok}'"))
            })?);
            start.push(i.min(dim.saturating_sub(1)));
            len.push(1);
        }
    }
    Ok((start, len))
}

/// One decoded region value → an `f64` (for CSV), optionally rescaled to physical units.
fn region_to_f64(d: &ArrayData, rescale: Option<(f64, f64)>) -> Vec<f64> {
    macro_rules! conv {
        ($v:expr) => {
            $v.iter()
                .map(|&x| {
                    let x = x as f64;
                    match rescale {
                        Some((s, i)) => s * x + i,
                        None => x,
                    }
                })
                .collect()
        };
    }
    match d {
        ArrayData::I16(v) => conv!(v),
        ArrayData::I32(v) => conv!(v),
        ArrayData::I64(v) => conv!(v),
        ArrayData::U16(v) => conv!(v),
        ArrayData::U32(v) => conv!(v),
        ArrayData::U64(v) => conv!(v),
        ArrayData::F32(v) => conv!(v),
        ArrayData::F64(v) => conv!(v),
    }
}

/// `tessera slice FILE BLOCK --index "z,:,:"` — pull a rectangular sub-region of an **array** block
/// (a 2-D plane, a 1-D line, or a point), decoding only the intersecting chunks. Emits the region as
/// CSV/TSV (last region axis = columns, the rest flattened to rows). `--physical` applies the
/// stored rescale (CT→HU, PET→Bq/mL).
pub fn slice(
    file: &Path,
    block: &str,
    index: &str,
    physical: bool,
    format: Format,
    out: &mut dyn Write,
) -> Result<()> {
    let (spec, blob) = open_array(file, block)?;
    let (start, len) = parse_index(index, &spec.shape)?;
    let region = tessera_io::array::decode_subset(&spec, &blob, &start, &len)?;

    let rescale = if physical {
        match (spec.rescale_slope, spec.rescale_intercept) {
            (Some(s), Some(i)) => Some((s, i)),
            _ => {
                return Err(tessera_core::Error::Invalid(
                    "--physical: this array carries no rescale_slope/intercept".into(),
                ))
            }
        }
    } else {
        None
    };
    let values = region_to_f64(&region, rescale);

    // Grid: the last region axis is the column count; everything before it flattens to rows.
    let cols = *len.last().unwrap_or(&1) as usize;
    let cols = cols.max(1);
    let sep = format.sep();
    for row in values.chunks(cols) {
        let line: Vec<String> = row.iter().map(fmt_f64).collect();
        writeln!(out, "{}", line.join(&sep.to_string())).map_err(tessera_core::Error::from)?;
    }
    Ok(())
}

/// Compact numeric render for slice CSV: integers without a trailing `.0`, floats to 6 sig-ish.
fn fmt_f64(v: &f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", *v as i64)
    } else {
        format!("{v}")
    }
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
        tree(&p, false, &mut buf).unwrap();
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
        ls(&p, None, false, &mut top).unwrap();
        assert!(String::from_utf8(top).unwrap().contains("events"));
        let mut cols = Vec::new();
        ls(&p, Some("events"), false, &mut cols).unwrap();
        let s = String::from_utf8(cols).unwrap();
        assert!(s.contains("ms") && s.contains("en"));
    }

    #[test]
    fn compact_reference_collapses_a_dicom_series_list() {
        // A DICOM-series `ingested_from`: many comma-joined slice paths under one dir.
        let dir = "/data/KSB/STUDY/VEN_CT_LUNG_0006";
        let refs: String = (1..=890)
            .map(|i| format!("{dir}/CT.0006.{i:04}.IMA"))
            .collect::<Vec<_>>()
            .join(",");

        // Default: collapse to "<first> (+N more)", and never dump the whole blob.
        let compact = compact_reference(&refs, false);
        assert!(compact.contains("(+889 more)"), "got: {compact}");
        assert!(compact.chars().count() < 120, "still noisy: {compact}");
        assert!(
            !compact.contains("0002.IMA"),
            "leaked the 2nd path: {compact}"
        );

        // --full is verbatim.
        assert_eq!(compact_reference(&refs, true), refs);

        // A single long path (>96 chars) middle-elides but keeps the filename tail.
        let one = format!(
            "{dir}/DUPLET-FAPI_07_CHERICO.CT.SPECIALS_DUPLET_PETCT.0006.0001.2026.06.24.20.07.21.880659.6623611.IMA"
        );
        assert!(one.chars().count() > 96);
        let e = compact_reference(&one, false);
        assert!(e.contains('…') && e.ends_with(".IMA"), "got: {e}");
    }

    #[test]
    fn common_dir_finds_the_shared_parent() {
        let paths = ["/a/b/c/one.IMA", "/a/b/c/two.IMA", "/a/b/c/three.IMA"];
        assert_eq!(common_dir(&paths), "/a/b/c");
        // Divergent parents collapse to the shared prefix.
        assert_eq!(common_dir(&["/a/b/x/one", "/a/b/y/two"]), "/a/b");
        assert_eq!(common_dir(&[]), "");
    }

    #[test]
    fn ls_sources_groups_a_multi_file_edge() {
        // 890 slices under one series dir → a grouped listing, not an 890-path comma blob.
        let dir = "/data/KSB/STUDY/VEN_CT_LUNG_0006";
        let refs: String = (1..=890)
            .map(|i| format!("{dir}/CT.0006.{i:04}.IMA"))
            .collect::<Vec<_>>()
            .join(",");

        // The source merkle root shows on the group header (the integrity link).
        let lines = source_lines(
            "ingested_from",
            &refs,
            Some("blake3:1a2b3c4d5e6f7890"),
            false,
        );
        // Header + 8 shown files + a "(+882 more)" footer = 10 lines.
        assert_eq!(lines.len(), 10, "{lines:#?}");
        assert_eq!(
            lines[0],
            format!("ingested_from <- 890 files in {dir}/  [blake3:1a2b3c4d5e6f…]")
        );
        assert_eq!(lines[1], "    CT.0006.0001.IMA"); // relative to the common dir
        assert_eq!(lines[9], "    … (+882 more, --full to list all)");

        // --full lists every file: header + 890 files, no footer.
        let full = source_lines("ingested_from", &refs, None, true);
        assert_eq!(full.len(), 891);
        assert!(full.last().unwrap().ends_with("CT.0006.0890.IMA"));

        // A single-file edge with no hash stays a bare one-liner.
        let one = source_lines("derived_from", "manifest:blake3:abcd", None, false);
        assert_eq!(one, vec!["derived_from <- manifest:blake3:abcd"]);
    }

    #[test]
    fn ls_and_tree_surface_schema_and_extra_nodes() {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::ProductBuilder;
        use tessera_io::{array::ArrayData, pack};
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.tsra");
        let spec = ArraySpec::new(vec![2, 2], "int16");
        let (bref, payload) =
            tessera_io::array::array_block("volume", &spec, &ArrayData::I16(vec![0, 1, 2, 3]))
                .unwrap();
        let mut b = ProductBuilder::new("recon", "R", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        // A representative extra blob (the shape #255 uses for the DICOM header).
        b.with_extra(
            "dicom_header",
            serde_json::json!({"0010,0010": {"vr": "PN", "value": ["X"]}}),
        );
        let sealed = b.seal().unwrap(); // seal embeds the recon schema (self-describing)
        pack(&sealed, &[payload], &p).unwrap();

        // Top-level ls lists the embedded schema + the extra namespace as navigable nodes.
        let mut top = Vec::new();
        ls(&p, None, false, &mut top).unwrap();
        let top = String::from_utf8(top).unwrap();
        assert!(top.contains("schema/"), "{top}");
        assert!(top.contains("extra/"), "{top}");

        // `ls FILE schema` shows the declared field roster; `ls FILE extra/<key>` dumps the blob.
        let mut sc = Vec::new();
        ls(&p, Some("schema"), false, &mut sc).unwrap();
        assert!(String::from_utf8(sc).unwrap().contains("modality"));
        let mut ex = Vec::new();
        ls(&p, Some("extra/dicom_header"), false, &mut ex).unwrap();
        assert!(String::from_utf8(ex).unwrap().contains("0010,0010"));

        // tree includes the schema + extra sub-trees.
        let mut t = Vec::new();
        tree(&p, false, &mut t).unwrap();
        let t = String::from_utf8(t).unwrap();
        assert!(t.contains("schema  (recon") && t.contains("extra"), "{t}");
    }

    #[test]
    fn parse_index_and_array_stats() {
        // Numpy-style index against a [4, 5, 6] array → (start, len) per axis.
        let shape = [4u64, 5, 6];
        assert_eq!(
            parse_index("1,:,:", &shape).unwrap(),
            (vec![1, 0, 0], vec![1, 5, 6])
        );
        assert_eq!(
            parse_index("0:2,:,3", &shape).unwrap(),
            (vec![0, 0, 3], vec![2, 5, 1])
        );
        // Negative index counts from the end (axis 0 len 4 → -1 = index 3).
        assert_eq!(
            parse_index("-1,:,:", &shape).unwrap(),
            (vec![3, 0, 0], vec![1, 5, 6])
        );
        // Wrong rank is a clear error, not a panic.
        assert!(parse_index("1,:", &shape).is_err());

        let (mn, mx, mean, std, n) = array_stats(&ArrayData::I16(vec![0, 2, 4, 6]));
        assert_eq!((mn, mx, n), (0.0, 6.0, 4));
        assert!((mean - 3.0).abs() < 1e-9 && (std - 5f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn slice_extracts_a_plane_from_a_sealed_array() {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::ProductBuilder;
        use tessera_io::{array::ArrayData, pack};
        // A 2×3 int16 array [[0,1,2],[10,11,12]] sealed as a `recon` volume.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.tsra");
        let spec = ArraySpec::new(vec![2, 3], "int16");
        let data = ArrayData::I16(vec![0, 1, 2, 10, 11, 12]);
        let (bref, payload) = tessera_io::array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "V", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], &p).unwrap();

        // Row 1 of the array → `10,11,12`.
        let mut buf = Vec::new();
        slice(&p, "volume", "1,:", false, Format::Csv, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap().trim(), "10,11,12");

        // stats reports the shape + value range.
        let mut s = Vec::new();
        stats(&p, "volume", &mut s).unwrap();
        let s = String::from_utf8(s).unwrap();
        assert!(s.contains("shape     [2, 3]"), "{s}");
        assert!(s.contains("min 0  max 12"), "{s}");

        // `read` on the array block is a clear typed error (table-only), not a decode panic.
        let err = read(
            ReadOpts {
                file: &p,
                block: "volume",
                columns: vec![],
                rows: None,
                all: false,
                limit: 20,
                format: Format::Csv,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("array block"), "{err}");
    }

    #[test]
    fn rowspec_resolves_open_negative_and_sugar() {
        let t = 100u64;
        // Open bounds: `91500:`-style (to end), `:N`, `:`.
        assert_eq!(RowSpec::parse_range("40:").unwrap().resolve(t), (40, 100));
        assert_eq!(RowSpec::parse_range(":40").unwrap().resolve(t), (0, 40));
        assert_eq!(RowSpec::parse_range(":").unwrap().resolve(t), (0, 100));
        // Negative-from-end: `-10:-1`, `-20:`.
        assert_eq!(RowSpec::parse_range("-10:-1").unwrap().resolve(t), (90, 99));
        assert_eq!(RowSpec::parse_range("-20:").unwrap().resolve(t), (80, 100));
        // Inverted range → empty window (Python-slice behaviour), never a panic.
        assert_eq!(RowSpec::parse_range("50:40").unwrap().resolve(t), (50, 50));
        // head / tail / at.
        assert_eq!(RowSpec::Head(10).resolve(t), (0, 10));
        assert_eq!(RowSpec::Tail(10).resolve(t), (90, 100));
        assert_eq!(RowSpec::At(-1).resolve(t), (99, 100));
        assert_eq!(RowSpec::At(0).resolve(t), (0, 1));
        // Clamping past the ends is safe.
        assert_eq!(RowSpec::parse_range("0:999").unwrap().resolve(t), (0, 100));
        assert_eq!(RowSpec::Tail(999).resolve(t), (0, 100));
        // A bare number is not a range (points at --at).
        assert!(RowSpec::parse_range("91500").is_err());
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
