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

/// Default row cap for text output when the user asks for no particular number — a preview length, not a
/// limit on what the tool can emit. Shared by `read`, `slice` and `project`.
pub const DEFAULT_GRID_ROWS: u64 = 20;

/// Resolve the row cap for a data-emitting verb. `None` means "no cap — write everything".
///
/// An explicit request (`--limit`, or `read`'s `--rows`/`--head`/`--tail`/`--at`) always wins, anywhere.
/// The **default** cap applies only when stdout is a **terminal**: a human at a prompt wants a preview,
/// but `> out.csv` or `| wc -l` is a script, and handing a script 20 of 4097 rows with nothing but a
/// stderr note is silent data loss rather than a courtesy. Found by the #387 review; `read` had shipped
/// that way since #391, so the rule is applied there too.
pub fn row_cap(explicit: Option<u64>, all: bool, interactive: bool) -> Option<u64> {
    match (all, explicit) {
        (true, _) => None,
        (false, Some(n)) => Some(n),
        (false, None) if interactive => Some(DEFAULT_GRID_ROWS),
        (false, None) => None,
    }
}

/// What `slice`/`project` write.
///
/// Deliberately separate from [`Format`] (the table formats) because the valid sets differ: an array
/// plane has no `ndjson` shape, and a table has no `npy`/`png`. A `ValueEnum` rather than a string parsed
/// at runtime, so clap rejects a typo at parse time and `--help` lists the values from one place instead
/// of a hand-maintained doc string that can drift (#387).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GridFormat {
    /// Comma-separated rows (default).
    Csv,
    /// Tab-separated rows.
    Tsv,
    /// One self-describing object: `{shape, dtype, values: [[row], …]}`.
    Json,
    /// NumPy `.npy` (float64) — the **lossless** path for analysis.
    Npy,
    /// 8-bit greyscale PNG — a **lossy preview**, windowed to 0–255. Not data.
    Png,
}

impl GridFormat {
    /// Text formats are line-oriented and honour the row cap. The binary ones always write the whole
    /// plane: a truncated `.npy` or `.png` is a **corrupt artifact**, not a preview.
    pub fn is_text(self) -> bool {
        !matches!(self, GridFormat::Npy | GridFormat::Png)
    }
    fn sep(self) -> char {
        if matches!(self, GridFormat::Tsv) {
            '\t'
        } else {
            ','
        }
    }
    fn name(self) -> &'static str {
        match self {
            GridFormat::Csv => "csv",
            GridFormat::Tsv => "tsv",
            GridFormat::Json => "json",
            GridFormat::Npy => "npy",
            GridFormat::Png => "png",
        }
    }
}

/// The dtype of a grid, which is **two** facts: what the array stores, and what these particular values
/// are. They diverge whenever the values were computed rather than read — `--physical` applies a rescale,
/// and `project --mode mean` averages — and a document that reported only the stored type would be
/// describing something other than the numbers beside it (#387 review).
pub struct GridDtype {
    stored: String,
    /// `true` when the emitted values are no longer the stored integers (a rescale or a reducing
    /// projection), so they are reported as `float64`.
    computed_float: bool,
}

impl GridDtype {
    /// Values read straight out of the array: emitted dtype == stored dtype.
    pub fn stored_as(dtype: &str) -> Self {
        GridDtype {
            stored: dtype.to_string(),
            computed_float: false,
        }
    }
    /// Values the CLI computed (rescale / mean / sum): emitted as `float64` whatever the array holds.
    pub fn computed_from(dtype: &str) -> Self {
        GridDtype {
            stored: dtype.to_string(),
            computed_float: true,
        }
    }
    /// The array's own dtype.
    fn stored(&self) -> &str {
        &self.stored
    }
    /// The dtype of the values actually emitted.
    fn emitted(&self) -> &str {
        if self.computed_float {
            "float64"
        } else {
            &self.stored
        }
    }
    /// Does the emitted grid hold floats? Drives the JSON number rendering.
    fn is_float(&self) -> bool {
        self.computed_float || self.stored.starts_with("float")
    }
}

/// How much of a grid to write, and how to window a preview image.
pub struct GridOpts {
    pub format: GridFormat,
    /// `Some(n)` caps **text** output at `n` rows; `None` means the caller gave no `--limit`, so a text
    /// format falls back to [`DEFAULT_GRID_ROWS`] and a binary one writes everything.
    pub limit: Option<u64>,
    /// `--all`: write every row (text formats; binary already does).
    pub all: bool,
    /// Is stdout a terminal? Decides whether the **default** cap applies at all — see [`row_cap`].
    pub interactive: bool,
    /// `png` only: an explicit intensity window `(lo, hi)`. `None` auto-windows on the plane's own
    /// finite min/max.
    pub window: Option<(f64, f64)>,
}

/// What [`write_grid`] emitted, so `main` can print notes to stderr without `nav` touching it.
#[derive(Debug)]
pub struct GridResult {
    pub shown: u64,
    pub total: u64,
    /// True when the row cap hid some rows.
    pub truncated: bool,
    /// A one-line advisory — currently the `png` "this is a lossy preview, windowed to …" note.
    pub note: Option<String>,
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

/// Trailing self-description for a table column line: `  · <unit> · ×<scale> · <description>`
/// (fd5 I1/I2, #307). Empty when the column carries no annotation, so legacy columns render
/// exactly as before (`name  dtype`).
fn column_annotation(c: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(u) = c.get("unit").and_then(Value::as_str) {
        parts.push(u.to_string());
    }
    if let Some(s) = c.get("scale").and_then(Value::as_f64) {
        parts.push(format!("×{s}"));
    }
    if let Some(d) = c.get("description").and_then(Value::as_str) {
        parts.push(d.to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  · {}", parts.join(" · "))
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
                        format!("{n:<10} {d}{}", column_annotation(c))
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
///
/// A file is "signed" if it carries **either** an embedded signature (ADR-0042 `aux/signatures/…`)
/// or a detached `<file>.tsra.sig.json` sidecar.
fn status_line(file: &Path, m: &tessera_core::Manifest, verified: bool) -> String {
    let known = m.schema.is_some() || SchemaRegistry::builtin().get(&m.product).is_some();
    let schema = if known {
        match tessera_core::validate_manifest(m) {
            Ok(()) => format!("schema={}✓", m.product),
            Err(_) => format!("schema={}✗", m.product),
        }
    } else {
        format!("schema={}(open-world)", m.product)
    };
    // `open` already re-verified the seal, so a sealed file's seal is valid here. The default badge
    // is `sealed` (seal only — payloads NOT re-hashed); `--verify` streams every payload first and
    // upgrades it to `verified✓` (the honest distinction the audit tool owes, #268).
    let sealed = match (verified, m.manifest_hash.is_some()) {
        (true, _) => "verified✓",
        (false, true) => "sealed",
        (false, false) => "unsealed",
    };
    let has_embedded = tessera_io::has_embedded_signature(file).unwrap_or(false);
    let has_detached = tessera_io::sign::sidecar_path(file).exists();
    let signed = if has_embedded || has_detached {
        " · signed"
    } else {
        ""
    };
    format!("product={} · {schema} · {sealed}{signed}", m.product)
}

/// `tessera tree FILE` — the whole hierarchy: root status, `meta` fields, every block (with its
/// columns / array spec), and `sources`, drawn with box characters.
pub fn tree(file: &Path, full: bool, verify: bool, out: &mut dyn Write) -> Result<()> {
    let mut r = Reader::open(file)?;
    // Deep-verify (opt-in): re-hash every block payload before rendering, so a corrupt file errors
    // out here rather than drawing a clean tree with a `sealed` badge (#268). Bounded RSS.
    if verify {
        r.verify_payloads(&file.display().to_string())?;
    }
    let m = r.manifest();
    let name = file
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<tsra>");
    writeln!(out, "{name}  ·  {}", status_line(file, m, verify))
        .map_err(tessera_core::Error::from)?;

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
    // Non-sealed aux members carried INSIDE the container (ADR-0042): the embedded signature +
    // `aux/provenance.json` (and anything else future producers stamp). Kept distinct from the
    // adjacent-sidecars node below so a reader immediately sees what's inside the one shareable
    // file vs what rides next to it on disk.
    let aux = r.aux_names();
    if !aux.is_empty() {
        let kids = aux.iter().map(|n| format!("aux/{n}")).collect();
        nodes.push(("aux".to_string(), kids));
    }

    // Adjacent sidecar files (outside the container AND outside the seal): the detached signature
    // (ADR-0037), and — when present — the field-encryption envelope (ADR-0041). Left here for the
    // operator who signed with `--sidecar` or an older Tessera. Shown so `tree` reflects the whole
    // on-disk product, not just the container.
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

/// The `inspect` render of the sealed ADR-0058 §1 **producer identity**.
///
/// [`ProducerRef::display`](tessera_core::ProducerRef::display) folds the build fields into a
/// `tool/version (commit)` one-liner — right for an inline mention, but it drops `git_repo` and
/// `dirty` entirely, so two of the three sealed build fields had no operator surface at all (#417).
/// `inspect` is that surface, so it shows every field the manifest actually carries: `tool/version`
/// on the header line, one indented line per present build field. A legacy bare-string producer
/// (pre-ADR-0058 manifests) round-trips verbatim — there is nothing more to show.
pub(crate) fn producer_lines(p: &tessera_core::ProducerRef) -> Vec<String> {
    let tessera_core::ProducerRef::Structured(s) = p else {
        return vec![format!("producer      {}", p.display())];
    };
    let mut lines = vec![format!("producer      {}/{}", s.tool, s.version)];
    for (k, v) in [
        ("git_repo", s.git_repo.clone()),
        ("git_commit", s.git_commit.clone()),
        ("dirty", s.dirty.map(|d| d.to_string())),
    ] {
        if let Some(v) = v {
            lines.push(format!("  {k:<12}{v}"));
        }
    }
    lines
}

/// The `inspect` render of the sealed ADR-0058 §2 **generation recipe** — the answer to "how was
/// this made?", which until #417 no CLI verb printed at all.
///
/// The header line summarises what the record holds (`N config keys`, and/or the `config_ref`
/// digest of a carried config block); the body is one line per config key. The bag is deliberately
/// **non-opinionated** (ADR-0058 §2), so the keys are rendered in the manifest's own order and are
/// never interpreted, validated, or re-grouped — display only. Default caps the body at
/// [`GENERATION_KEYS_SHOWN`] keys and elides long values, mirroring how `sources` collapses; `--full`
/// prints every key and the full digest.
///
/// Resolving a `config_ref` to the bytes of the block it points at is a separate, larger job
/// (ADR-0058 §2 / #417) — this prints the digest, not the payload.
pub(crate) fn generation_lines(g: &tessera_core::Generation, full: bool) -> Vec<String> {
    let mut head = Vec::new();
    if !g.config.is_empty() {
        let plural = if g.config.len() == 1 { "" } else { "s" };
        head.push(format!("{} config key{plural}", g.config.len()));
    }
    if let Some(r) = &g.config_ref {
        let digest = if full { r.clone() } else { short_hash(r) };
        head.push(format!("config_ref {digest}"));
    }
    if head.is_empty() {
        // A sealed-but-empty record is still state the operator should see; printing nothing would
        // read as "this product has no recipe", which is a different fact.
        head.push("(empty)".to_string());
    }
    let mut lines = vec![format!("generation    {}", head.join(" · "))];
    let show = if full {
        g.config.len()
    } else {
        g.config.len().min(GENERATION_KEYS_SHOWN)
    };
    for (k, v) in g.config.iter().take(show) {
        lines.push(format!("  - {k:<22} {}", compact_value(v, full)));
    }
    if g.config.len() > show {
        lines.push(format!(
            "    … (+{} more, --full to list all)",
            g.config.len() - show
        ));
    }
    lines
}

/// How many `generation.config` keys `inspect` prints before collapsing — the same cap `ls sources`
/// uses for a multi-file provenance edge, so the two summaries read alike.
const GENERATION_KEYS_SHOWN: usize = 8;

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
    let mut r = Reader::open(file)?;
    let aux_names = r.aux_names();
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
            // Non-sealed aux members carried inside the container (ADR-0042): embedded signature,
            // ingest provenance, anything future producers stamp. Navigable via `ls FILE aux`.
            if !aux_names.is_empty() {
                writeln!(out, "aux/  ({} members)", aux_names.len())
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
        Some(p) if is_extra_path(p) => write_extra(m, p, out),
        Some(p) if p == "aux" || p == "aux/" => {
            // List the embedded aux members carried inside the container (ADR-0042). No sizes are
            // shown — an aux member is opaque JSON / arbitrary bytes; `ls FILE aux/<name>` reads it.
            for n in &aux_names {
                writeln!(out, "aux/{n}").map_err(tessera_core::Error::from)?;
            }
            Ok(())
        }
        Some(p) if p.starts_with("aux/") => {
            let key = &p["aux/".len()..];
            // read_aux surfaces the exact bytes; for the two canonical members (signature +
            // provenance) the bytes are JSON — pretty-printed for the reader.
            let bytes = r.read_aux(key)?;
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(v) => writeln!(out, "{}", serde_json::to_string_pretty(&v)?)
                    .map_err(tessera_core::Error::from)?,
                Err(_) => {
                    // Not JSON — write the raw bytes as-is (a future aux member may be non-JSON).
                    out.write_all(&bytes).map_err(tessera_core::Error::from)?;
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
    /// The user's explicit `--limit`, if any. `None` falls back to the shared [`row_cap`] rule, where the
    /// default applies **only** to an interactive terminal.
    pub limit: Option<u64>,
    /// Is stdout a terminal? See [`row_cap`] — piping or redirecting gets every row.
    pub interactive: bool,
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
    // `extra/*` is manifest metadata rather than a block, so it matches neither the non-table-block
    // guard below nor any table prefix — it used to fall through to `logical_table` and surface
    // "no blocks for prefix 'extra/…'" (#303). Serve it as JSON through the same renderer `ls` uses, so
    // both navigation verbs address one namespace. Not row-oriented, hence never truncated.
    if is_extra_path(opts.block) {
        write_extra(r.manifest(), opts.block, out)?;
        return Ok(ReadResult {
            shown: 1,
            total: 1,
            truncated: false,
        });
    }
    // `read` is table-only. If the target names a non-table block (array volume, blob, index), fail
    // with a clear pointer instead of the opaque "missing field columns" from the table decoder
    // (#253/#268). Covers Array/Blob/ChunkIndex; a multi-block table prefix like `events` won't
    // exact-match a single block, so it falls through to the logical-table path.
    if let Some(b) = r.manifest().blocks.iter().find(|b| b.name == opts.block) {
        if b.kind != BlockKind::Table {
            let hint = match b.kind {
                BlockKind::Array => "use `tsra stats` / `tsra slice` / `tsra project`",
                BlockKind::Blob => "use `tsra extract` for its raw bytes",
                _ => "use `tsra ls` / `tsra inspect`",
            };
            return Err(tessera_core::Error::Invalid(format!(
                "'{}' is a {} block ({}), not a table — `read` is for tables. {hint}, or \
                 `tsra ls {} {}` for its spec.",
                opts.block,
                format!("{:?}", b.kind).to_lowercase(),
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
    // The default cap is a preview for a human, not a limit on what a script may receive (#387 review):
    // `read … > out.csv` used to hand over 20 of 4097 rows with only a stderr note. An explicit
    // `--rows`/`--head`/`--tail`/`--at`/`--limit` still applies everywhere.
    let cap = row_cap(opts.limit, opts.all, opts.interactive);
    let (lo, hi) = match opts.rows {
        Some(spec) => spec.resolve(total),
        None => (0, cap.map_or(total, |n| n.min(total))),
    };
    // Only the default-cap path is a silent truncation worth warning about.
    let truncated = opts.rows.is_none() && hi < total;
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

/// `tessera pyramid FILE BLOCK --out OUT` — build a **multiscale pyramid** of the array `BLOCK`: the
/// full-resolution level plus successive 2× max-downsampled levels (`BLOCK/1`, `BLOCK/2`, …, each with
/// its `WorldFrame::at_level` affine), sealed as a new `recon` product `derived_from` the source. The
/// coarse levels answer overview/zoom without decoding the full volume (#260 phase 2). Returns the
/// number of levels written (including L0).
pub fn build_pyramid(file: &Path, block: &str, levels: Option<u32>, out: &Path) -> Result<usize> {
    let (spec, blob) = open_array(file, block)?;
    if spec.shape.len() != 3 {
        return Err(tessera_core::Error::Invalid(
            "pyramid: needs a 3-D array (downsampling is defined for volumes)".into(),
        ));
    }
    // Source identity for the derived_from edge + inherited name/timestamp.
    let src = Reader::open(file)?;
    let sm = src.manifest();
    let parent_hash = sm.manifest_hash.clone().unwrap_or_default();
    let (name, timestamp) = (sm.name.clone(), sm.timestamp.clone());

    let mut cur_spec = spec.clone();
    let mut data = tessera_io::array::decode(&spec, &blob)?;
    let mut blocks: Vec<(tessera_core::block::BlockRef, tessera_io::BlockPayload)> = Vec::new();
    // Level 0 — the full-resolution volume.
    blocks.push(tessera_io::array::array_block(block, &cur_spec, &data)?);

    let cap = levels.unwrap_or(8);
    let mut level = 0u32;
    while level < cap {
        let Some((ds_spec, ds_data)) = tessera_io::array::downsample_max_3d(&cur_spec, &data)
        else {
            break;
        };
        level += 1;
        let lname = format!("{block}/{level}");
        blocks.push(tessera_io::array::array_block(&lname, &ds_spec, &ds_data)?);
        cur_spec = ds_spec;
        data = ds_data;
        // Stop once the coarsest level is a single overview tile.
        if cur_spec.shape.iter().copied().max().unwrap_or(0) <= 64 {
            break;
        }
    }

    let mut b =
        tessera_core::ProductBuilder::new(&*sm.product, name, "multiscale pyramid", timestamp);
    let mut payloads = Vec::with_capacity(blocks.len());
    for (bref, payload) in blocks {
        b.add_block_ref(bref);
        payloads.push(payload);
    }
    b.add_source(
        tessera_core::provenance::Source::new("derived_from", &parent_hash)
            .with_content_hash(&parent_hash),
    );
    let sealed = b.seal()?;
    tessera_io::pack(&sealed, &payloads, out)?;
    Ok((level + 1) as usize)
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
        ArrayData::I8(v) => reduce!(v),
        ArrayData::U8(v) => reduce!(v),
        ArrayData::I16(v) => reduce!(v),
        ArrayData::I32(v) => reduce!(v),
        ArrayData::I64(v) => reduce!(v),
        ArrayData::U16(v) => reduce!(v),
        ArrayData::U32(v) => reduce!(v),
        ArrayData::U64(v) => reduce!(v),
        ArrayData::F32(v) => reduce!(v),
        ArrayData::F64(v) => reduce!(v),
        // f16/bool have no `as f64` cast — go through the lossless f64 view.
        ArrayData::F16(_) | ArrayData::Bool(_) => {
            let v = d.as_f64();
            reduce!(v)
        }
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
        ArrayData::I8(v) => conv!(v),
        ArrayData::U8(v) => conv!(v),
        ArrayData::I16(v) => conv!(v),
        ArrayData::I32(v) => conv!(v),
        ArrayData::I64(v) => conv!(v),
        ArrayData::U16(v) => conv!(v),
        ArrayData::U32(v) => conv!(v),
        ArrayData::U64(v) => conv!(v),
        ArrayData::F32(v) => conv!(v),
        ArrayData::F64(v) => conv!(v),
        // f16/bool have no `as f64` cast — go through the lossless f64 view.
        ArrayData::F16(_) | ArrayData::Bool(_) => {
            let v = d.as_f64();
            conv!(v)
        }
    }
}

/// `tessera slice FILE BLOCK --index "z,:,:"` — pull a rectangular sub-region of an **array** block
/// (a 2-D plane, a 1-D line, or a point), decoding only the intersecting chunks. Emits the region as
/// CSV/TSV (last region axis = columns, the rest flattened to rows). `--physical` applies the
/// stored rescale (CT→HU, PET→Bq/mL).
/// Invert a 3×3 matrix (cofactor method), or `None` if singular.
fn inv3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let id = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * id,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * id,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * id,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * id,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * id,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * id,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * id,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * id,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * id,
        ],
    ])
}

/// Round an affine-resolved (bounded) coordinate to an integer voxel index — truncation is intended.
#[allow(clippy::cast_possible_truncation)]
fn round_index(v: f64) -> i64 {
    v.round() as i64
}

/// Resolve a world `(L,P,S)` mm point to the nearest voxel index via the **inverse** of the stored
/// voxel→world affine (`index = R⁻¹·(world − t)`). `None` if the array has no affine or it is singular.
fn world_to_index(
    wf: &tessera_core::block::array::WorldFrame,
    world: [f64; 3],
) -> Option<[i64; 3]> {
    let a = &wf.affine; // row-major 3×4 [R | t]
    let r = [[a[0], a[1], a[2]], [a[4], a[5], a[6]], [a[8], a[9], a[10]]];
    let t = [a[3], a[7], a[11]];
    let inv = inv3(&r)?;
    let d = [world[0] - t[0], world[1] - t[1], world[2] - t[2]];
    let mul = |row: &[f64; 3]| row[0] * d[0] + row[1] * d[1] + row[2] * d[2];
    Some([
        round_index(mul(&inv[0])),
        round_index(mul(&inv[1])),
        round_index(mul(&inv[2])),
    ])
}

/// Parse `"L,P,S"` into an mm point.
fn parse_world(s: &str) -> Result<[f64; 3]> {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| {
            x.trim()
                .parse::<f64>()
                .map_err(|_| tessera_core::Error::Invalid(format!("--world: bad coordinate '{x}'")))
        })
        .collect::<Result<_>>()?;
    <[f64; 3]>::try_from(v).map_err(|_| {
        tessera_core::Error::Invalid("--world expects 3 mm coordinates `L,P,S`".into())
    })
}

pub fn slice(
    file: &Path,
    block: &str,
    index: Option<&str>,
    world: Option<&str>,
    physical: bool,
    opts: &GridOpts,
    out: &mut dyn Write,
) -> Result<GridResult> {
    let (spec, blob) = open_array(file, block)?;
    let (start, len) = match (index, world) {
        (Some(ix), _) => parse_index(ix, &spec.shape)?,
        (None, Some(w)) => {
            let wf = spec.world_frame.as_ref().ok_or_else(|| {
                tessera_core::Error::Invalid(
                    "--world: this array is in index space (no affine) — use --index".into(),
                )
            })?;
            if spec.shape.len() != 3 {
                return Err(tessera_core::Error::Invalid(
                    "--world addressing requires a 3-D array".into(),
                ));
            }
            let idx = world_to_index(wf, parse_world(w)?).ok_or_else(|| {
                tessera_core::Error::Invalid("--world: the array affine is singular".into())
            })?;
            // Resolve to that single voxel (clamped in-bounds); print its value.
            let start: Vec<u64> = idx
                .iter()
                .zip(&spec.shape)
                .map(|(&i, &dim)| i.clamp(0, dim as i64 - 1).max(0) as u64)
                .collect();
            (start, vec![1, 1, 1])
        }
        (None, None) => {
            return Err(tessera_core::Error::Invalid(
                "slice needs --index or --world".into(),
            ))
        }
    };
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

    // Grid: the last region axis is the column count; everything before it flattens to rows. `--physical`
    // means the values are rescaled floats, not the stored integers.
    let dtype = if physical {
        GridDtype::computed_from(&spec.dtype)
    } else {
        GridDtype::stored_as(&spec.dtype)
    };
    write_grid(&values, &len, &dtype, opts, out)
}

/// Reduce mode for [`project`].
#[derive(Clone, Copy)]
enum ProjMode {
    /// Maximum-intensity projection (MIP) — the classic PET/CT overview.
    Max,
    /// Mean along the axis.
    Mean,
    /// Sum along the axis.
    Sum,
}

impl ProjMode {
    fn parse(s: &str) -> Result<ProjMode> {
        match s {
            "max" | "mip" => Ok(ProjMode::Max),
            "mean" | "avg" => Ok(ProjMode::Mean),
            "sum" => Ok(ProjMode::Sum),
            other => Err(tessera_core::Error::Invalid(format!(
                "unknown --mode '{other}' (expected max | mean | sum)"
            ))),
        }
    }
}

/// Reduce a row-major N-D array `values` (shape `shape`) along `axis` by `mode`, dropping that axis.
/// Returns `(out_shape, out_values)`.
fn project_axis(
    values: &[f64],
    shape: &[u64],
    axis: usize,
    mode: ProjMode,
) -> (Vec<u64>, Vec<f64>) {
    let n = shape.len();
    let mut strides = vec![1usize; n];
    for i in (0..n.saturating_sub(1)).rev() {
        strides[i] = strides[i + 1] * shape[i + 1] as usize;
    }
    let ax_len = shape[axis].max(1) as usize;
    let out_shape: Vec<u64> = shape
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != axis)
        .map(|(_, &d)| d)
        .collect();
    let out_n: usize = out_shape.iter().map(|&d| d as usize).product();
    let init = match mode {
        ProjMode::Max => f64::NEG_INFINITY,
        _ => 0.0,
    };
    let mut out = vec![init; out_n.max(1)];
    for (flat, &v) in values.iter().enumerate() {
        // Output flat index = input coords with the projected axis removed (row-major).
        let mut of = 0usize;
        let mut os = 1usize;
        for i in (0..n).rev() {
            if i == axis {
                continue;
            }
            let coord = (flat / strides[i]) % shape[i] as usize;
            of += coord * os;
            os *= shape[i] as usize;
        }
        match mode {
            ProjMode::Max => out[of] = out[of].max(v),
            ProjMode::Mean | ProjMode::Sum => out[of] += v,
        }
    }
    if matches!(mode, ProjMode::Mean) {
        for o in &mut out {
            *o /= ax_len as f64;
        }
    }
    (out_shape, out)
}

/// `tessera project FILE BLOCK --axis <name|idx> --mode max|mean|sum` — collapse an **array** block
/// along one axis into a lower-D image (a 3-D volume → a 2-D projection). MIP (`max`) over the z axis
/// is the classic PET/CT overview. Emits the result as CSV/TSV (last surviving axis = columns);
/// `--physical` applies the rescale.
pub fn project(
    file: &Path,
    block: &str,
    axis: &str,
    mode: &str,
    physical: bool,
    opts: &GridOpts,
    out: &mut dyn Write,
) -> Result<GridResult> {
    let (spec, blob) = open_array(file, block)?;
    let mode = ProjMode::parse(mode)?;
    // Resolve the axis by name (from `spec.axes`) or by index.
    let ax = spec
        .axes
        .iter()
        .position(|a| a == axis)
        .or_else(|| axis.parse::<usize>().ok())
        .filter(|&a| a < spec.shape.len())
        .ok_or_else(|| {
            tessera_core::Error::Invalid(format!(
                "--axis '{axis}' not found (axes: {}; or 0..{})",
                spec.axes.join(","),
                spec.shape.len()
            ))
        })?;
    let data = tessera_io::array::decode(&spec, &blob)?;
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
    let values = region_to_f64(&data, rescale);
    let (out_shape, out_vals) = project_axis(&values, &spec.shape, ax, mode);
    // `max` picks existing samples, so they keep the stored dtype. `mean` averages and `sum` can leave the
    // stored type's range, so both are reported as the floats they were computed as — and `--physical`
    // rescales whatever the mode.
    let dtype = if physical || !matches!(mode, ProjMode::Max) {
        GridDtype::computed_from(&spec.dtype)
    } else {
        GridDtype::stored_as(&spec.dtype)
    };
    write_grid(&out_vals, &out_shape, &dtype, opts, out)
}

/// Does this path address the manifest's `extra/` namespace (the fd5 extension fields)?
///
/// `extra/*` is **manifest metadata, not a block**, which is why `read` used to fall through to the
/// logical-table path and surface its internal "no blocks for prefix" error (#303).
fn is_extra_path(p: &str) -> bool {
    p == "extra" || p == "extra/" || p.starts_with("extra/")
}

/// Render the `extra/` namespace: the key listing for `extra` itself, or one key's value as pretty JSON.
///
/// Shared by `ls` and `read` so the two navigation verbs address exactly the same namespace (#303) —
/// `ls` could reach the preserved DICOM header while `read` could not, which is confusing for a first
/// user exploring a product, and the fix is to make the error impossible rather than better-worded.
fn write_extra(
    m: &tessera_core::manifest::Manifest,
    path: &str,
    out: &mut dyn Write,
) -> Result<()> {
    if path == "extra" || path == "extra/" {
        if m.extra.is_empty() {
            writeln!(out, "(no extra fields)").map_err(tessera_core::Error::from)?;
        }
        for (k, v) in &m.extra {
            let kind = match v {
                Value::Object(o) => format!("object, {} keys", o.len()),
                Value::Array(a) => format!("array, {} items", a.len()),
                Value::String(_) => "string".into(),
                other => other.to_string(),
            };
            writeln!(out, "{k}  ({kind})").map_err(tessera_core::Error::from)?;
        }
        return Ok(());
    }
    let key = &path["extra/".len()..];
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

/// Write a 2-D grid of values (row-major, `cols` = the last axis) in the requested format.
///
/// ONE place, so `slice` and `project` cannot drift on rendering, capping or windowing — they used to
/// carry a copy of the row loop each (#387).
fn write_grid(
    values: &[f64],
    shape: &[u64],
    dtype: &GridDtype,
    opts: &GridOpts,
    out: &mut dyn Write,
) -> Result<GridResult> {
    // An explicit cap on a binary format is a flag that would do nothing, which is its own foot-gun —
    // say so rather than ignore it.
    if !opts.format.is_text() && opts.limit.is_some() {
        return Err(tessera_core::Error::Invalid(format!(
            "--limit applies to text output; --format {} always writes the full plane",
            opts.format.name()
        )));
    }
    let cols = shape.last().copied().unwrap_or(1).max(1) as usize;
    let total = values.len().div_ceil(cols) as u64;
    // Binary formats never cap (a truncated artifact is corrupt, not a preview); text formats follow the
    // shared rule, where the *default* only bites an interactive terminal.
    let cap = if opts.format.is_text() {
        row_cap(opts.limit, opts.all, opts.interactive).map_or(total, |n| n.min(total))
    } else {
        total
    };
    let shown_vals = &values[..(cap as usize * cols).min(values.len())];
    let mut res = GridResult {
        shown: cap,
        total,
        truncated: cap < total,
        note: None,
    };

    match opts.format {
        GridFormat::Csv | GridFormat::Tsv => {
            let sep = opts.format.sep().to_string();
            for row in shown_vals.chunks(cols) {
                let line: Vec<String> = row.iter().map(fmt_f64).collect();
                writeln!(out, "{}", line.join(&sep)).map_err(tessera_core::Error::from)?;
            }
        }
        GridFormat::Json => {
            // Self-describing AND honest about what it is: `shape` is the FULL region, so a saved
            // document can never masquerade as the complete plane; `rows_emitted`/`truncated` say what
            // was actually written; `dtype` describes the VALUES and `source_dtype` the stored array.
            let rows: Vec<Value> = shown_vals
                .chunks(cols)
                .map(|r| Value::Array(r.iter().map(|v| json_num(*v, dtype.is_float())).collect()))
                .collect();
            let doc = serde_json::json!({
                "shape": [total, cols as u64],
                "rows_emitted": rows.len() as u64,
                "truncated": res.truncated,
                "dtype": dtype.emitted(),
                "source_dtype": dtype.stored(),
                "values": rows,
            });
            writeln!(out, "{}", serde_json::to_string(&doc)?).map_err(tessera_core::Error::from)?;
        }
        GridFormat::Npy => write_npy(shown_vals, total, cols as u64, out)?,
        GridFormat::Png => {
            let (lo, hi) = write_png(shown_vals, cols, dtype.stored(), opts.window, out)?;
            res.note = Some(format!(
                "png is a lossy 8-bit preview (window {lo} … {hi}, source dtype {}) — \
                 use --format npy for the data",
                dtype.stored()
            ));
        }
    }
    Ok(res)
}

/// A grid cell as JSON, keyed off the **emitted** dtype rather than the value's shape.
///
/// For an integer grid an integral value renders as an integer — the rule [`fmt_f64`] already applies to
/// CSV/TSV, so the two text formats agree and an `int16` array does not come back as `0.0`. For a float
/// grid the float-ness is preserved (`3.0` stays `3.0`, `-0.0` keeps its sign), because collapsing a
/// float column to integers whenever it happens to hold whole numbers would contradict the `dtype` the
/// same document declares.
///
/// NaN and ±inf become JSON `null`: JSON cannot spell them, and `null` is the standard mapping.
#[allow(clippy::cast_possible_truncation)]
fn json_num(v: f64, float_grid: bool) -> Value {
    if !float_grid && v.fract() == 0.0 && v.abs() < 1e15 {
        return Value::Number((v as i64).into());
    }
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

/// Write a 2-D NumPy `.npy` v1.0 array: the magic + version, a padded header dict, then the values.
///
/// Always `'<f8'` (little-endian float64), because this whole code path is `f64`: `region_to_f64` converts
/// on the way in, shared with the CSV and `stats` paths. So `f8` is *exactly* what the CLI computed rather
/// than a widening introduced here.
///
/// **The fidelity caveat, stated precisely:** that is exact for every dtype whose values fit in an `f64`
/// mantissa — `int8/16/32`, `uint8/16/32`, `float16/32`, `bool` — i.e. every dtype a Tessera array is
/// likely to hold, and `--physical` output is floating-point regardless. It is **not** exact for `int64` /
/// `uint64` magnitudes above 2^53, where `f64` cannot represent every integer. Emitting a native `'<i8'`
/// descr would not fix that, only relabel it, because the precision is already gone before this function
/// sees the values; a faithful integer path means carrying `ArrayData` through instead of `f64`, which is
/// tracked separately. For full-fidelity 64-bit integers use the Python bindings or `tessera export`.
///
/// The header is space-padded so `10 + header_len` is a multiple of 64 — the alignment numpy's own writer
/// produces, and what its reader expects.
fn write_npy(values: &[f64], rows: u64, cols: u64, out: &mut dyn Write) -> Result<()> {
    let dict = format!("{{'descr': '<f8', 'fortran_order': False, 'shape': ({rows}, {cols}), }}");
    let pad = (64 - ((10 + dict.len() + 1) % 64)) % 64;
    let mut header = dict.into_bytes();
    header.extend(std::iter::repeat_n(b' ', pad));
    header.push(b'\n');
    let len = u16::try_from(header.len())
        .map_err(|_| tessera_core::Error::Invalid("npy header too long".into()))?;
    out.write_all(b"\x93NUMPY\x01\x00")
        .map_err(tessera_core::Error::from)?;
    out.write_all(&len.to_le_bytes())
        .map_err(tessera_core::Error::from)?;
    out.write_all(&header).map_err(tessera_core::Error::from)?;
    for v in values {
        out.write_all(&v.to_le_bytes())
            .map_err(tessera_core::Error::from)?;
    }
    Ok(())
}

/// Write an 8-bit greyscale PNG preview, windowing `[lo, hi]` onto black..white. Returns the window
/// actually used.
///
/// This is **not data**: 8 bits cannot hold a CT Hounsfield range, let alone a float activity map, so the
/// mapping is lossy by construction and `npy` is the lossless path. The window used and the source dtype
/// are written into `tEXt` chunks so a preview that has been copied out of context stays self-describing
/// and cannot be mistaken for the values later.
fn write_png(
    values: &[f64],
    cols: usize,
    dtype: &str,
    window: Option<(f64, f64)>,
    out: &mut dyn Write,
) -> Result<(f64, f64)> {
    let rows = values.len().div_ceil(cols.max(1));
    if rows == 0 || cols == 0 {
        return Err(tessera_core::Error::Invalid(
            "png: nothing to write (empty plane)".into(),
        ));
    }
    // Auto-window on the plane's own finite min/max — the honest default for an unknown modality, and
    // what makes `--format png` a one-liner. Non-finite samples are excluded from the window.
    let (lo, hi) = window.unwrap_or_else(|| {
        let finite = || values.iter().copied().filter(|v| v.is_finite());
        let lo = finite().fold(f64::INFINITY, f64::min);
        let hi = finite().fold(f64::NEG_INFINITY, f64::max);
        if lo.is_finite() && hi.is_finite() {
            (lo, hi)
        } else {
            (0.0, 0.0)
        }
    });
    let span = hi - lo;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let grey: Vec<u8> = values
        .iter()
        .map(|&v| {
            if !v.is_finite() || span <= 0.0 {
                // A flat plane, or a non-finite sample, has no contrast to show. Black, and said out
                // loud in the tEXt comment below — because a black pixel is otherwise indistinguishable
                // from a legitimate sample sitting at the window's low end.
                0
            } else {
                (((v - lo) / span).clamp(0.0, 1.0) * 255.0).round() as u8
            }
        })
        .collect();

    let (w, h) = (
        u32::try_from(cols).map_err(|_| tessera_core::Error::Invalid("png: too wide".into()))?,
        u32::try_from(rows).map_err(|_| tessera_core::Error::Invalid("png: too tall".into()))?,
    );
    let mut enc = png::Encoder::new(out, w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let text = |e: png::EncodingError| tessera_core::Error::Invalid(format!("png: {e}"));
    enc.add_text_chunk("Software".into(), "tessera".into())
        .map_err(text)?;
    enc.add_text_chunk("tessera:window".into(), format!("{lo},{hi}"))
        .map_err(text)?;
    enc.add_text_chunk("tessera:source_dtype".into(), dtype.to_string())
        .map_err(text)?;
    enc.add_text_chunk(
        "Comment".into(),
        "lossy 8-bit preview of a Tessera array; not the data \
         (use `tsra slice|project --format npy`). NaN/inf samples render as black, \
         indistinguishable from the window's low end."
            .into(),
    )
    .map_err(text)?;
    let mut writer = enc.write_header().map_err(text)?;
    writer.write_image_data(&grey).map_err(text)?;
    writer.finish().map_err(text)?;
    Ok((lo, hi))
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

/// Convert a (sliced) column to per-row JSON values. Floats render via their **native**
/// shortest round-trip `Display` (so an `f32` shows `0.01`, not its widened-`f64` expansion);
/// non-finite floats (NaN/±inf) have no JSON encoding → null (CSV shows `nan`, ndjson `null`).
/// Bool columns render as JSON `true`/`false`, Utf8 as JSON strings.
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
        ColumnData::Bool(v) => v.iter().map(|x| Value::from(*x)).collect(),
        ColumnData::Utf8(v) => v.iter().map(|x| Value::from(x.as_str())).collect(),
        // NULL renders as JSON null — distinct from a NaN float, which also renders null but
        // means "not a number", not "no value". ndjson shows `null`; CSV shows `nan` via
        // `csv_cell`, matching how the non-finite float case already reads.
        ColumnData::Nullable { values, validity } => col_to_values(values)
            .into_iter()
            .zip(validity)
            .map(|(v, &ok)| if ok { v } else { Value::Null })
            .collect(),
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
                    ..Default::default()
                },
                Column {
                    name: "en".into(),
                    dtype: "f4".into(),
                    codec: None,
                    ..Default::default()
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
    fn column_annotation_renders_unit_scale_description() {
        // Annotated column → self-describing suffix (fd5 I1/I2, #307).
        let annotated = serde_json::json!({
            "name": "en", "dtype": "i2", "unit": "keV", "scale": 0.1,
            "description": "Calibrated per-photon energy"
        });
        assert_eq!(
            column_annotation(&annotated),
            "  · keV · ×0.1 · Calibrated per-photon energy"
        );
        // Bare column → empty suffix, so legacy columns render exactly as before.
        let bare = serde_json::json!({ "name": "ms", "dtype": "u4" });
        assert_eq!(column_annotation(&bare), "");
        // Unit-only is fine (no scale/description).
        let unit_only = serde_json::json!({ "name": "ax", "dtype": "u1", "unit": "1" });
        assert_eq!(column_annotation(&unit_only), "  · 1");
    }

    #[test]
    fn annotated_column_shows_in_block_children() {
        let spec = serde_json::json!({
            "columns": [
                { "name": "en", "dtype": "i2", "unit": "keV",
                  "description": "energy", "scale": 0.1 },
                { "name": "ms", "dtype": "u4" }
            ],
            "rows": 4
        });
        let lines = block_children(&BlockKind::Table, &spec);
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("keV") && lines[0].contains("×0.1"),
            "{}",
            lines[0]
        );
        assert!(
            !lines[1].contains('·'),
            "bare column stays plain: {}",
            lines[1]
        );
    }

    #[test]
    fn tree_renders_root_meta_block_columns() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.tsra");
        sample(&p);
        let mut buf = Vec::new();
        tree(&p, false, false, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("product=listmode"));
        assert!(s.contains("schema=listmode")); // known schema; ✓/✗ depends on field completeness
        assert!(s.contains("meta"));
        assert!(s.contains("modality"));
        assert!(s.contains("events"));
        assert!(s.contains("ms")); // a column leaf
    }

    /// #268 part 1: the default `tree` badge is `sealed` (seal only — the seal `open` verified),
    /// and `--verify` re-hashes every payload and upgrades the badge to `verified✓`.
    #[test]
    fn tree_verify_upgrades_the_badge_from_sealed_to_verified() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.tsra");
        sample(&p);

        let mut buf = Vec::new();
        tree(&p, false, false, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("· sealed") && !s.contains("verified"), "{s}");

        let mut buf = Vec::new();
        tree(&p, false, true, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("verified✓"), "{s}");
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

    /// #417: the sealed producer's build fields must all be reachable. `ProducerRef::display()`
    /// folds them into `tool/version (commit)`, which silently loses `git_repo` and `dirty` — so
    /// `inspect` renders each present field on its own line instead.
    #[test]
    fn producer_lines_surface_every_sealed_build_field() {
        let mut p = tessera_core::Producer::new("ge-listmode-daq", "3.2");
        p.git_commit = Some("0855f5f".into());
        p.git_repo = Some("vig-os/ge-daq".into());
        p.dirty = Some(false);
        let lines = producer_lines(&tessera_core::ProducerRef::Structured(p));
        assert_eq!(lines[0], "producer      ge-listmode-daq/3.2");
        let body = lines[1..].join("\n");
        for want in [
            "git_repo",
            "vig-os/ge-daq",
            "git_commit",
            "0855f5f",
            "dirty",
            "false",
        ] {
            assert!(body.contains(want), "missing {want} in:\n{body}");
        }

        // A producer with no build fields is just the header line — no empty scaffolding.
        let bare = producer_lines(&tessera_core::ProducerRef::Structured(
            tessera_core::Producer::new("sim", "0.1"),
        ));
        assert_eq!(bare, vec!["producer      sim/0.1".to_string()]);

        // A pre-ADR-0058 bare string round-trips verbatim.
        let legacy = producer_lines(&tessera_core::ProducerRef::Legacy("tessera/0.0.0".into()));
        assert_eq!(legacy, vec!["producer      tessera/0.0.0".to_string()]);
    }

    /// #417: the sealed generation recipe gets an operator surface — a summary header plus the
    /// config bag, collapsing like `sources` does until `--full`.
    #[test]
    fn generation_lines_summarise_then_collapse_until_full() {
        // Inline config + a config_ref: the header states both, the digest is shortened by default.
        let mut g = tessera_core::Generation::default()
            .with("energy_window_keV", serde_json::json!("425-650"))
            .with("seed", serde_json::json!(7));
        g.config_ref = Some("blake3:0123456789abcdef0123456789abcdef".into());
        let lines = generation_lines(&g, false);
        assert_eq!(
            lines[0],
            "generation    2 config keys · config_ref blake3:0123456789ab…"
        );
        assert!(lines[1].contains("energy_window_keV") && lines[1].contains("\"425-650\""));
        assert!(lines[2].contains("seed") && lines[2].contains('7'));
        // --full spells the digest out.
        assert!(generation_lines(&g, true)[0].contains("blake3:0123456789abcdef0123456789abcdef"));

        // A bag larger than the cap collapses with a footer; --full lists every key.
        let big = (0..12).fold(tessera_core::Generation::default(), |acc, i| {
            acc.with(format!("k{i:02}"), serde_json::json!(i))
        });
        let capped = generation_lines(&big, false);
        assert_eq!(capped[0], "generation    12 config keys");
        assert_eq!(
            capped.len(),
            1 + GENERATION_KEYS_SHOWN + 1,
            "header + 8 + footer"
        );
        assert!(capped
            .last()
            .unwrap()
            .contains("(+4 more, --full to list all)"));
        assert_eq!(generation_lines(&big, true).len(), 1 + 12);

        // #462: config_ref AND a bag over the cap AND --full, together — the combination the
        // per-feature assertions above never exercise as one render. `--full` must spell the digest
        // out AND list every key, with the header still reporting both halves.
        let mut both = (0..12).fold(tessera_core::Generation::default(), |acc, i| {
            acc.with(format!("k{i:02}"), serde_json::json!(i))
        });
        both.config_ref = Some("blake3:0123456789abcdef0123456789abcdef".into());
        let full = generation_lines(&both, true);
        assert_eq!(
            full[0],
            "generation    12 config keys · config_ref blake3:0123456789abcdef0123456789abcdef",
            "--full must not shorten the digest when a bag is present too"
        );
        assert_eq!(full.len(), 1 + 12, "--full lists every key, no footer");
        assert!(full.last().unwrap().contains("k11"));
        // ...and the default render of the SAME record collapses the bag but still names the digest.
        let capped_both = generation_lines(&both, false);
        assert_eq!(
            capped_both[0],
            "generation    12 config keys · config_ref blake3:0123456789ab…"
        );
        assert_eq!(capped_both.len(), 1 + GENERATION_KEYS_SHOWN + 1);
        assert!(capped_both
            .last()
            .unwrap()
            .contains("(+4 more, --full to list all)"));

        // config_ref alone — the large/bit-faithful vendor-config case (ADR-0058 §2).
        let only_ref = tessera_core::Generation::default().with_config_ref("blake3:deadbeefcafe00");
        assert_eq!(
            generation_lines(&only_ref, false),
            vec!["generation    config_ref blake3:deadbeefcafe…".to_string()]
        );

        // A sealed-but-empty record is still reported — silence would read as "no recipe".
        assert_eq!(
            generation_lines(&tessera_core::Generation::default(), false),
            vec!["generation    (empty)".to_string()]
        );
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
        tree(&p, false, false, &mut t).unwrap();
        let t = String::from_utf8(t).unwrap();
        assert!(t.contains("schema  (recon") && t.contains("extra"), "{t}");
    }

    #[test]
    fn build_pyramid_writes_downsampled_levels() {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::ProductBuilder;
        use tessera_io::{array::ArrayData, pack};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("v.tsra");
        // An 8×8×8 int16 volume (ramp) → seal.
        let spec = ArraySpec::new(vec![8, 8, 8], "int16");
        let data = ArrayData::I16((0..512).map(|k| k as i16).collect());
        let (bref, payload) = tessera_io::array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "V", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], &src).unwrap();

        let out = dir.path().join("pyr.tsra");
        let n = build_pyramid(&src, "volume", None, &out).unwrap();
        assert!(
            n >= 2,
            "expected the full-res level + ≥1 downsample, got {n}"
        );

        // L0 is the original 8³; L1 is the 4³ 2×-downsample.
        let r = Reader::open(&out).unwrap();
        let names: Vec<&str> = r
            .manifest()
            .blocks
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert!(
            names.contains(&"volume") && names.contains(&"volume/1"),
            "{names:?}"
        );
        // derived_from the source is recorded.
        assert!(r
            .manifest()
            .sources
            .iter()
            .any(|s| s.role == "derived_from"));
        // The pyramid product verifies (seal + every block digest).
        let (s1, blob1) = open_array(&out, "volume/1").unwrap();
        assert_eq!(s1.shape, vec![4, 4, 4]);
        let _ = tessera_io::array::decode(&s1, &blob1).unwrap();
    }

    #[test]
    fn project_axis_reduces_along_an_axis() {
        // 2×3 array [[0,1,2],[10,11,12]] (row-major, shape [2,3]).
        let v = vec![0.0, 1.0, 2.0, 10.0, 11.0, 12.0];
        let shape = [2u64, 3];
        // Max over axis 0 (rows) → the max of each column: [10,11,12].
        let (os, out) = project_axis(&v, &shape, 0, ProjMode::Max);
        assert_eq!(os, vec![3]);
        assert_eq!(out, vec![10.0, 11.0, 12.0]);
        // Sum over axis 1 (cols) → row sums: [0+1+2, 10+11+12] = [3, 33].
        let (os, out) = project_axis(&v, &shape, 1, ProjMode::Sum);
        assert_eq!(os, vec![2]);
        assert_eq!(out, vec![3.0, 33.0]);
        // Mean over axis 1 → [1, 11].
        let (_, out) = project_axis(&v, &shape, 1, ProjMode::Mean);
        assert_eq!(out, vec![1.0, 11.0]);
        assert!(ProjMode::parse("mip").is_ok() && ProjMode::parse("nope").is_err());
    }

    #[test]
    fn world_to_index_inverts_the_affine() {
        use tessera_core::block::array::WorldFrame;
        // 2 mm isotropic voxels, LPS, with a translation — a diagonal affine.
        let wf = WorldFrame {
            affine: [
                2.0, 0.0, 0.0, -100.0, //
                0.0, 2.0, 0.0, -50.0, //
                0.0, 0.0, 2.0, 10.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "scanner".into(),
        };
        // world (0,0,10) → index ((0+100)/2, (0+50)/2, (10-10)/2) = (50, 25, 0).
        assert_eq!(world_to_index(&wf, [0.0, 0.0, 10.0]), Some([50, 25, 0]));
        // A point that rounds: world (-99,-49,11) → (0.5, 0.5, 0.5) → rounds to (1,1,1)... check.
        assert_eq!(world_to_index(&wf, [-98.0, -48.0, 12.0]), Some([1, 1, 1]));
        // Singular affine → None.
        let sing = WorldFrame {
            affine: [0.0; 12],
            ..wf
        };
        assert_eq!(world_to_index(&sing, [1.0, 2.0, 3.0]), None);
        assert!(parse_world("1,2,3").is_ok() && parse_world("1,2").is_err());
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

    /// Default text-CSV grid options — the shape almost every test wants.
    fn csv_grid() -> GridOpts {
        GridOpts {
            format: GridFormat::Csv,
            limit: None,
            all: true,
            interactive: false,
            window: None,
        }
    }

    /// Seal a small 2-D int16 array as a `recon` volume and hand back its path.
    fn sealed_grid(dir: &std::path::Path, shape: Vec<u64>, vals: Vec<i16>) -> std::path::PathBuf {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::ProductBuilder;
        use tessera_io::{array::ArrayData, pack};
        let p = dir.join("g.tsra");
        let spec = ArraySpec::new(shape, "int16");
        let (bref, payload) =
            tessera_io::array::array_block("volume", &spec, &ArrayData::I16(vals)).unwrap();
        let mut b = ProductBuilder::new("recon", "G", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], &p).unwrap();
        p
    }

    /// **#387** — array reads emitted CSV only, which is a foot-gun on a real volume: a 512×512 plane is
    /// a 512-wide CSV spew, and an analyst wants numpy or an image. `json` carries the shape and dtype
    /// with the numbers; `npy` is the lossless path.
    ///
    /// The expectations here are derived **independently of our own writer** — the `.npy` bytes against
    /// the NumPy format spec (magic, `'<f8'` descr, 64-byte header alignment, little-endian payload) and
    /// the PNG by decoding it with the `png` crate's *reader*. A test that merely echoed whatever we
    /// emitted would pass for a wrong encoder.
    #[test]
    fn slice_writes_json_npy_and_png() {
        let dir = tempfile::tempdir().unwrap();
        // 2×3: [[0,1,2],[10,11,12]]
        let p = sealed_grid(dir.path(), vec![2, 3], vec![0, 1, 2, 10, 11, 12]);
        let run = |fmt: GridFormat, window: Option<(f64, f64)>| {
            let mut buf = Vec::new();
            let opts = GridOpts {
                format: fmt,
                limit: None,
                all: true,
                interactive: false,
                window,
            };
            let res = slice(&p, "volume", Some(":,:"), None, false, &opts, &mut buf).unwrap();
            (buf, res)
        };

        // ── json: self-describing, and the values are the array ──
        let (buf, _) = run(GridFormat::Json, None);
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["shape"], serde_json::json!([2, 3]));
        assert_eq!(v["dtype"], "int16");
        assert_eq!(v["values"], serde_json::json!([[0, 1, 2], [10, 11, 12]]));

        // ── npy: checked against the NumPy format spec, not against our own output ──
        let (buf, _) = run(GridFormat::Npy, None);
        assert_eq!(&buf[..6], b"\x93NUMPY", "magic");
        assert_eq!(&buf[6..8], &[1, 0], "version 1.0");
        let hlen = u16::from_le_bytes([buf[8], buf[9]]) as usize;
        assert_eq!(
            (10 + hlen) % 64,
            0,
            "numpy requires 64-byte header alignment"
        );
        let header = std::str::from_utf8(&buf[10..10 + hlen]).unwrap();
        assert!(header.contains("'descr': '<f8'"), "{header}");
        assert!(header.contains("'fortran_order': False"), "{header}");
        assert!(header.contains("'shape': (2, 3)"), "{header}");
        assert!(header.ends_with('\n'), "the header must end with a newline");
        let body = &buf[10 + hlen..];
        assert_eq!(body.len(), 6 * 8, "6 float64 values");
        let got: Vec<f64> = body
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(got, vec![0.0, 1.0, 2.0, 10.0, 11.0, 12.0]);

        // ── png: decoded back with the png crate; window + dtype recorded in tEXt ──
        let (buf, res) = run(GridFormat::Png, None);
        let decoder = png::Decoder::new(std::io::Cursor::new(&buf));
        let mut reader = decoder.read_info().unwrap();
        let info = reader.info();
        assert_eq!((info.width, info.height), (3, 2), "cols × rows");
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        assert_eq!(info.color_type, png::ColorType::Grayscale);
        let texts: Vec<(String, String)> = info
            .uncompressed_latin1_text
            .iter()
            .map(|t| (t.keyword.clone(), t.text.clone()))
            .collect();
        let get = |k: &str| {
            texts
                .iter()
                .find(|(kw, _)| kw == k)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| panic!("missing tEXt '{k}' in {texts:?}"))
        };
        // the window ACTUALLY used, so a preview copied out of context stays self-describing
        assert_eq!(get("tessera:window"), "0,12");
        assert_eq!(get("tessera:source_dtype"), "int16");
        assert!(get("Comment").contains("not the data"), "{texts:?}");
        // auto-window maps min→black and max→white
        let mut pixels = vec![0u8; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        let px = &pixels[..frame.buffer_size()];
        assert_eq!(px[0], 0, "the minimum (0) is black");
        assert_eq!(px[5], 255, "the maximum (12) is white");
        // and the caller is told it is lossy
        let note = res.note.expect("png must report that it is a preview");
        assert!(note.contains("lossy"), "{note}");
        assert!(
            note.contains("npy"),
            "the note must point at the lossless path: {note}"
        );

        // ── an explicit window overrides, and is what gets recorded ──
        let (buf, _) = run(GridFormat::Png, Some((0.0, 24.0)));
        let mut reader = png::Decoder::new(std::io::Cursor::new(&buf))
            .read_info()
            .unwrap();
        let recorded = reader
            .info()
            .uncompressed_latin1_text
            .iter()
            .find(|t| t.keyword == "tessera:window")
            .map(|t| t.text.clone())
            .unwrap();
        assert_eq!(recorded, "0,24");
        let mut pixels = vec![0u8; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        // 12 of a 0..24 window is mid-grey, not white any more
        assert_eq!(pixels[..frame.buffer_size()][5], 128);
    }

    /// **#387 review** — the default cap must not silently truncate a **script**. `read … > out.csv` had
    /// handed over 20 of 4097 rows since #391 with nothing but a stderr note, and the first version of
    /// this PR copied that rule to `slice`/`project`. The default now applies only to an interactive
    /// terminal; an explicit request applies anywhere.
    #[test]
    fn the_default_row_cap_only_applies_to_an_interactive_terminal() {
        // the rule itself, in one place both read and the grid writer consult
        assert_eq!(
            row_cap(None, false, true),
            Some(DEFAULT_GRID_ROWS),
            "a terminal previews"
        );
        assert_eq!(
            row_cap(None, false, false),
            None,
            "a pipe or redirect gets everything"
        );
        assert_eq!(
            row_cap(Some(5), false, false),
            Some(5),
            "an explicit cap applies when piped"
        );
        assert_eq!(row_cap(Some(5), false, true), Some(5), "…and at a terminal");
        assert_eq!(
            row_cap(None, true, true),
            None,
            "--all lifts it even at a terminal"
        );
        assert_eq!(
            row_cap(Some(5), true, true),
            None,
            "--all wins over an explicit cap"
        );

        // and end-to-end through `slice`: 30 rows, more than the 20-row default
        let dir = tempfile::tempdir().unwrap();
        let p = sealed_grid(dir.path(), vec![30, 2], (0..60).collect());
        let run = |interactive: bool, limit: Option<u64>| {
            let mut buf = Vec::new();
            let opts = GridOpts {
                format: GridFormat::Csv,
                limit,
                all: false,
                interactive,
                window: None,
            };
            let res = slice(&p, "volume", Some(":,:"), None, false, &opts, &mut buf).unwrap();
            (String::from_utf8(buf).unwrap().lines().count(), res)
        };
        let (lines, res) = run(true, None);
        assert_eq!(lines, 20, "a terminal gets the preview");
        assert!(res.truncated);
        let (lines, res) = run(false, None);
        assert_eq!(lines, 30, "a redirect must NOT lose 10 rows silently");
        assert!(!res.truncated);
        let (lines, _) = run(false, Some(3));
        assert_eq!(lines, 3, "an explicit --limit still applies when piped");
    }

    /// **#387 review** — the JSON document must describe *itself*, or a saved file can lie: the stored
    /// dtype is not the dtype of computed values, and a capped write is not the whole plane.
    #[test]
    fn json_describes_the_values_it_actually_emitted() {
        let dir = tempfile::tempdir().unwrap();
        // 4 rows × 2, values 0..8, with a rescale so `--physical` produces genuine floats
        let p = {
            use tessera_core::block::array::ArraySpec;
            use tessera_core::ProductBuilder;
            use tessera_io::{array::ArrayData, pack};
            let path = dir.path().join("r.tsra");
            let spec = ArraySpec::new(vec![4, 2], "int16").with_rescale(0.5, -1.0);
            let (bref, payload) =
                tessera_io::array::array_block("volume", &spec, &ArrayData::I16((0..8).collect()))
                    .unwrap();
            let mut b = ProductBuilder::new("recon", "R", "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(bref);
            pack(&b.seal().unwrap(), &[payload], &path).unwrap();
            path
        };
        let doc = |physical: bool, limit: Option<u64>| {
            let mut buf = Vec::new();
            let opts = GridOpts {
                format: GridFormat::Json,
                limit,
                all: false,
                interactive: false,
                window: None,
            };
            slice(&p, "volume", Some(":,:"), None, physical, &opts, &mut buf).unwrap();
            serde_json::from_slice::<Value>(&buf).unwrap()
        };

        // stored values: the dtype IS the array's, and integers stay integers
        let v = doc(false, None);
        assert_eq!(v["dtype"], "int16");
        assert_eq!(v["source_dtype"], "int16");
        assert_eq!(v["shape"], serde_json::json!([4, 2]));
        assert_eq!(v["rows_emitted"], 4);
        assert_eq!(v["truncated"], false);
        assert_eq!(v["values"][0], serde_json::json!([0, 1]));

        // --physical: the emitted values are floats, so THAT is the dtype; the stored one is kept
        // separately. 0 → -1.0 and 1 → -0.5 under (slope 0.5, intercept -1).
        let v = doc(true, None);
        assert_eq!(v["dtype"], "float64", "a rescaled read emits floats");
        assert_eq!(
            v["source_dtype"], "int16",
            "…and still says what was stored"
        );
        assert_eq!(v["values"][0], serde_json::json!([-1.0, -0.5]));
        // float-ness is preserved rather than collapsed: 2 → 0.0, not 0
        assert_eq!(v["values"][1][0].as_f64(), Some(0.0));
        assert!(
            v["values"][1][0].is_f64(),
            "3.0 must stay a float in a float grid"
        );

        // a capped write says so, and `shape` stays the FULL region so it cannot pose as complete
        let v = doc(false, Some(2));
        assert_eq!(
            v["shape"],
            serde_json::json!([4, 2]),
            "the full region shape"
        );
        assert_eq!(v["rows_emitted"], 2);
        assert_eq!(v["truncated"], true);
        assert_eq!(v["values"].as_array().unwrap().len(), 2);
    }

    /// **#387 review** — `project --mode mean` computes averages, so the emitted dtype is `float64` even
    /// though the array holds int16; `max` picks existing samples, so it keeps the stored dtype.
    #[test]
    fn a_reducing_projection_reports_the_dtype_it_computed() {
        let dir = tempfile::tempdir().unwrap();
        let p = sealed_grid(dir.path(), vec![2, 2, 2], vec![0, 1, 2, 3, 4, 5, 6, 7]);
        let doc = |mode: &str| {
            let mut buf = Vec::new();
            let opts = GridOpts {
                format: GridFormat::Json,
                limit: None,
                all: true,
                interactive: false,
                window: None,
            };
            project(&p, "volume", "0", mode, false, &opts, &mut buf).unwrap();
            serde_json::from_slice::<Value>(&buf).unwrap()
        };
        assert_eq!(doc("max")["dtype"], "int16", "max picks stored samples");
        let mean = doc("mean");
        assert_eq!(mean["dtype"], "float64", "mean computes floats");
        assert_eq!(mean["source_dtype"], "int16");
        // (0+4)/2 = 2.0 — and it stays a float, matching the declared dtype
        assert!(mean["values"][0][0].is_f64(), "{mean}");
        // `sum` can leave the stored type's range, so it is reported as computed too
        assert_eq!(doc("sum")["dtype"], "float64");
    }

    /// **#387** — the size guard. Text output caps like `read` does; binary output never does, because a
    /// truncated `.npy`/`.png` is a corrupt artifact rather than a preview — and an explicit `--limit`
    /// with a binary format is therefore an **error**, since a flag that silently does nothing is its own
    /// foot-gun.
    #[test]
    fn text_grid_output_caps_and_binary_output_refuses_a_cap() {
        let dir = tempfile::tempdir().unwrap();
        // 5 rows × 2 cols
        let p = sealed_grid(dir.path(), vec![5, 2], (0..10).collect());
        let run = |opts: &GridOpts| {
            let mut buf = Vec::new();
            slice(&p, "volume", Some(":,:"), None, false, opts, &mut buf)
                .map(|res| (String::from_utf8_lossy(&buf).to_string(), res))
        };
        let text = |limit: Option<u64>, all: bool| GridOpts {
            format: GridFormat::Csv,
            limit,
            all,
            interactive: false,
            window: None,
        };

        // an explicit cap wins, and the result reports the truncation for main's stderr note
        let (out, res) = run(&text(Some(2), false)).unwrap();
        assert_eq!(out.lines().count(), 2);
        assert_eq!((res.shown, res.total, res.truncated), (2, 5, true));

        // --all lifts it
        let (out, res) = run(&text(None, true)).unwrap();
        assert_eq!(out.lines().count(), 5);
        assert!(!res.truncated);

        // no flags → the shared default, and NOT truncated here because 5 < 20
        let (out, res) = run(&text(None, false)).unwrap();
        assert_eq!(out.lines().count(), 5);
        assert!(!res.truncated);
        // (this case relies on DEFAULT_GRID_ROWS exceeding the 5 rows above, which it does at 20)

        // binary formats ignore no flag silently: an explicit --limit is refused, naming the reason
        for fmt in [GridFormat::Npy, GridFormat::Png] {
            let err = run(&GridOpts {
                format: fmt,
                limit: Some(2),
                all: false,
                interactive: false,
                window: None,
            })
            .unwrap_err();
            let msg = format!("{err}");
            assert!(msg.contains("--limit applies to text output"), "{msg}");
            assert!(
                msg.contains(fmt.name()),
                "the error must name the format: {msg}"
            );
        }

        // …but --all with a binary format is consistent (it asks for everything and gets everything)
        let (_, res) = run(&GridOpts {
            format: GridFormat::Npy,
            limit: None,
            all: true,
            interactive: false,
            window: None,
        })
        .unwrap();
        assert_eq!((res.shown, res.total, res.truncated), (5, 5, false));
    }

    /// **#387** — `project` shares the one grid writer, so it gets every format and the same cap without
    /// a second copy of the row loop (the two verbs each carried one before).
    #[test]
    fn project_shares_the_grid_writer() {
        let dir = tempfile::tempdir().unwrap();
        // 2×2×2 where a max-projection over axis 0 gives [[4,5],[6,7]]
        let p = sealed_grid(dir.path(), vec![2, 2, 2], vec![0, 1, 2, 3, 4, 5, 6, 7]);
        let mut buf = Vec::new();
        let opts = GridOpts {
            format: GridFormat::Json,
            limit: None,
            all: true,
            interactive: false,
            window: None,
        };
        project(&p, "volume", "0", "max", false, &opts, &mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["values"], serde_json::json!([[4, 5], [6, 7]]));
        assert_eq!(v["dtype"], "int16");

        // and the cap refusal reaches project too, from the same place
        let err = project(
            &p,
            "volume",
            "0",
            "max",
            false,
            &GridOpts {
                format: GridFormat::Png,
                limit: Some(1),
                all: false,
                interactive: false,
                window: None,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("--limit applies to text output"));
    }

    /// **#303** — `ls` and `read` disagreed about what is addressable. The full preserved DICOM header
    /// lands in `extra/dicom_header` (an object, ~167 keys) and `ls FILE extra/dicom_header` dumps it,
    /// but `read FILE extra/dicom_header` died with the internal
    /// `container: logical_table: no blocks for prefix 'extra/dicom_header'`.
    ///
    /// The cause is that `extra/*` is **not a block at all** — it is manifest metadata — so it never
    /// matched the non-table-block guard above and fell through to the logical-table path. A first user
    /// exploring what is inside a product hits it immediately. `read` now serves it as JSON, so the two
    /// navigation verbs address the same namespace and the error is impossible rather than better-worded.
    #[test]
    fn read_reaches_extra_object_blocks_as_json() {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::ProductBuilder;
        use tessera_io::{array::ArrayData, pack};
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.tsra");
        let spec = ArraySpec::new(vec![2, 2], "int16");
        let data = ArrayData::I16(vec![1, 2, 3, 4]);
        let (bref, payload) = tessera_io::array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "H", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        b.with_extra(
            "dicom_header",
            serde_json::json!({"0008,0060": "CT", "0018,0050": "1.25"}),
        );
        b.with_extra("note", serde_json::json!("a string value"));
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], &p).unwrap();

        let read_to_string = |block: &str| {
            let mut buf = Vec::new();
            read(
                ReadOpts {
                    file: &p,
                    block,
                    columns: vec![],
                    rows: None,
                    all: false,
                    limit: None,
                    interactive: false,
                    format: Format::Csv,
                },
                &mut buf,
            )
            .map(|_| String::from_utf8(buf).unwrap())
        };

        // the object case: valid JSON carrying the tags, NOT a logical_table error.
        let out = read_to_string("extra/dicom_header")
            .unwrap_or_else(|e| panic!("read must reach extra/ blocks: {e}"));
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["0008,0060"], "CT");
        assert_eq!(v["0018,0050"], "1.25");

        // a scalar extra value works too.
        let out = read_to_string("extra/note").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&out).unwrap(),
            "a string value"
        );

        // `extra` itself lists the keys rather than erroring.
        let out = read_to_string("extra").unwrap();
        assert!(out.contains("dicom_header"), "{out}");

        // an unknown key names the available ones instead of leaking the table decoder's message.
        let err = format!("{}", read_to_string("extra/nope").unwrap_err());
        assert!(err.contains("dicom_header"), "{err}");
        assert!(
            !err.contains("logical_table"),
            "must not surface the internal table error: {err}"
        );
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
        slice(
            &p,
            "volume",
            Some("1,:"),
            None,
            false,
            &csv_grid(),
            &mut buf,
        )
        .unwrap();
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
                limit: None,
                interactive: false,
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
                limit: Some(2),
                interactive: false,
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
                limit: Some(2),
                interactive: false,
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

    /// `b1`/`str` columns (#354) render as JSON booleans and strings — not as 0/1 or a debug
    /// string — and `csv_cell` passes them through so the CSV path shows `true` / `annih511`.
    #[test]
    fn bool_and_utf8_render_as_json_bool_and_string() {
        let flags = col_to_values(&ColumnData::Bool(vec![true, false]));
        assert_eq!(flags, vec![Value::Bool(true), Value::Bool(false)]);
        assert_eq!(csv_cell(&flags[0]), "true");

        let origins = col_to_values(&ColumnData::Utf8(vec![
            "annih511".to_string(),
            "prompt_nuclear".to_string(),
        ]));
        assert_eq!(
            origins,
            vec![
                Value::String("annih511".into()),
                Value::String("prompt_nuclear".into())
            ]
        );
        // csv_cell goes through `Value::to_string()` for non-numbers → JSON-quoted.
        assert_eq!(csv_cell(&origins[0]), "\"annih511\"");
    }
}
