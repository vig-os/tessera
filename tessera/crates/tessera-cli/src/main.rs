//! `tessera` — pack / unpack / verify / inspect `.tsra` products (ROADMAP P4, #205).
//!
//! A thin shell over `tessera-core` (format/spine) + `tessera-io` (container). Every command
//! that opens a `.tsra` verifies its magic + manifest seal; `verify` additionally checks every
//! block's stored bytes against its recorded digest.

mod bench;
mod nav;
#[cfg(feature = "sql")]
mod sql;
mod trust;
mod version;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tessera_core::SchemaRegistry;
use tessera_ingest::{engine, spec as ingest_spec};
use tessera_io::{pack_dir, parse_byte_size, unpack, Reader, WriteConfig};

/// Cloud-URL prefixes the `cloud` feature recognises. Used to detect a URL-shaped argument and
/// route it through `tessera_io::open_url` instead of the local file path.
#[cfg(feature = "cloud")]
const CLOUD_SCHEMES: &[&str] = &["s3://", "http://", "https://"];

/// Returns `Some(url_str)` when `arg` looks like a cloud URL handled by `tessera_io::open_url`,
/// else `None` (caller falls back to local-path `Reader::open`).
#[cfg(feature = "cloud")]
fn cloud_url(arg: &std::path::Path) -> Option<String> {
    let s = arg.to_str()?;
    if CLOUD_SCHEMES.iter().any(|p| s.starts_with(p)) {
        Some(s.to_string())
    } else {
        None
    }
}

/// Open a `.tsra` from either a local file path or — when the `cloud` feature is enabled — an
/// `s3://` / `http(s)://` URL. Returns a boxed [`tessera_core::Manifest`] reader trampoline: the
/// callers (`inspect`/`verify`) only need the manifest + per-block reads, both of which the
/// trampoline forwards without leaking the concrete `Reader<R>` generic parameter to the CLI.
fn open_local_or_url(arg: &std::path::Path) -> tessera_core::Result<Box<dyn TsraSource>> {
    #[cfg(feature = "cloud")]
    if let Some(url) = cloud_url(arg) {
        let r = tessera_io::open_url(&url)?;
        return Ok(Box::new(r));
    }
    Ok(Box::new(Reader::open(arg)?))
}

/// CLI-only erased view over a `Reader<R>` — narrows the trait-object surface to the few
/// methods `inspect` / `verify` / `extract` actually need (manifest access, per-block read,
/// bounded-memory stream). Hides the concrete `R` (local `File` vs `ObjectStoreReader`)
/// behind a single boxed handle.
trait TsraSource {
    fn manifest(&self) -> &tessera_core::Manifest;
    fn read_block_by_name(&mut self, name: &str) -> tessera_core::Result<Vec<u8>>;
    fn block_names(&self) -> Vec<String>;
    /// Bounded-memory copy of a block's bytes into `w`, digest-verified after the last byte.
    /// Delegates to [`Reader::stream_block`] — preserves the same integrity contract: on
    /// `Err(Integrity)` the writer already saw the unverified bytes, so callers must stage to
    /// a temp path and only expose the final destination on `Ok`.
    fn stream_block_to(
        &mut self,
        name: &str,
        w: &mut dyn std::io::Write,
    ) -> tessera_core::Result<u64>;
}

impl<R: std::io::Read + std::io::Seek> TsraSource for Reader<R> {
    fn manifest(&self) -> &tessera_core::Manifest {
        Reader::manifest(self)
    }
    fn read_block_by_name(&mut self, name: &str) -> tessera_core::Result<Vec<u8>> {
        Reader::read_block(self, name)
    }
    fn block_names(&self) -> Vec<String> {
        Reader::block_names(self)
    }
    fn stream_block_to(
        &mut self,
        name: &str,
        mut w: &mut dyn std::io::Write,
    ) -> tessera_core::Result<u64> {
        // `Reader::stream_block` is generic over `W: Write` (implicit `Sized`), so re-borrow the
        // trait object as `&mut &mut dyn Write` — the mutable-reference impl of `Write` is itself
        // `Sized`, which keeps the generic happy without changing the underlying writer.
        Reader::stream_block(self, name, &mut w)
    }
}

/// Grouped top-level help (#249) — clap 4 doesn't group subcommands natively, so we render a
/// hand-authored command reference by command family via a custom `help_template`. Per-command
/// detail (`tsra help <cmd>`) still comes from each variant's own `///` doc + positional
/// descriptions (#243), unchanged. The `every_subcommand_is_grouped_in_help` test guards drift.
const HELP_TEMPLATE: &str = "\
{about}

{usage-heading} {usage}

Inspect & navigate:
  inspect     Manifest summary (id, product, blocks, hashes)
  verify      Verify integrity (magic, seal, every block digest)
  schema      Validate against the embedded product schema (--json dumps it)
  tree        Render the .tsra as a navigable hierarchy
  ls          List one node's children (meta / a block / sources)
  read        Read table data as CSV/TSV/NDJSON (cross-block)
  stats       Numeric overview of an array block (shape, dtype, value range)
  slice       Pull a plane/line/point of an array block as CSV (--index z,:,:)
  project     Collapse an array along an axis → 2-D image (--mode max|mean|sum)
  pyramid     Build a multiscale pyramid of an array block → a new .tsra
  export      Emit a FAIR discovery record (JSON to stdout)

Query (needs --features sql):
  sql         Run SQL (DataFusion) over a table block

Ingest & pack:
  ingest      Ingest a vendor file (or a declarative --spec) into a sealed .tsra
  pack        Pack an exploded dir (manifest.json + blocks/) into a sealed .tsra
  unpack      Explode a .tsra into a directory
  extract     Extract one block's raw bytes (digest-verified)

Versioning (content-addressed repo):
  init        Initialize a content-addressed repository for CoW versioning
  import      Import a sealed .tsra as the first version of its lineage
  commit      Commit a new version (reuses unchanged blocks by digest)
  log         Show a lineage's version history
  diff        Diff two versions (blocks + metadata)
  seal        Export a version to a standalone .tsra with its history
  publish     Export a history-free standalone .tsra for publication
  forget      Forget a lineage (objects reclaimed by gc)
  gc          Reclaim objects unreachable from any ref

Distribution (needs --features cloud):
  push        Push a sealed .tsra to an OCI registry
  pull        Pull a .tsra OCI artifact from a registry

Signing & trust:
  keygen      Generate an ed25519 keypair
  trust       Manage the trust store of public keys verify-sig accepts
  sign        Sign a sealed .tsra (embeds aux/signatures/<key_id>.sig.json; --sidecar for detached)
  verify-sig  Verify a sealed .tsra against its signature (embedded first, sidecar fallback)

Confidentiality (crypto-shred):
  reidentify  Recover a crypto-shred product's identity with a recipient private key (ADR-0047)
  shred       Permanently remove a product's identity envelope (aux/identity)

Diagnostics:
  bench       Bench the write engine on this host (throughput + peak RSS)

Run `tsra help <command>` for details, flags, and what to pass.

Options:
{options}";

#[derive(Parser)]
#[command(
    name = "tessera",
    version,
    about = "Tessera FAIR data-product CLI",
    help_template = HELP_TEMPLATE
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a `.tsra` manifest summary (id, product, blocks, hashes).
    ///
    /// Prints a human summary of the manifest's identity, product, blocks, and hashes.
    ///
    /// With the `cloud` feature enabled, `file` also accepts `s3://<bucket>/<key>` or
    /// `http(s)://<host>/<key>` — the manifest is read via range-GET over the wire.
    Inspect {
        /// The `.tsra` to summarise (or an `s3://` / `http(s)://` URL with the `cloud` feature).
        file: PathBuf,
        /// Print provenance references in full (a DICOM series' `ingested_from` lists every slice
        /// path); default collapses a multi-file edge to `<first> (+N more)`.
        #[arg(long)]
        full: bool,
    },
    /// Verify a `.tsra`'s integrity (magic, seal, every block digest).
    ///
    /// Opens + fully verifies the container (magic, manifest seal, every block digest).
    /// Exits 0 if valid.
    ///
    /// With the `cloud` feature, `file` also accepts `s3://` / `http(s)://` URLs.
    Verify {
        /// The `.tsra` to verify (or an `s3://` / `http(s)://` URL with the `cloud` feature).
        file: PathBuf,
    },
    /// Render a `.tsra` as a navigable hierarchy tree.
    ///
    /// Renders the root status (product · schema · sealed · signed), `meta` fields, every block
    /// with its columns / array spec, and `sources`.
    Tree {
        /// The `.tsra` to render.
        file: PathBuf,
        /// Print provenance references in full instead of collapsing a multi-file edge.
        #[arg(long)]
        full: bool,
    },
    /// List one node's children (top level, `meta`, a block, or `sources`).
    ///
    /// No PATH = top level (`meta`, blocks, `sources`); `PATH=meta` = metadata fields;
    /// `PATH=<block>` = a table's columns or an array's spec; `PATH=sources` = edges.
    Ls {
        /// The `.tsra` to list.
        file: PathBuf,
        /// Node to list (a block name, `meta`, or `sources`). Omit for the top level.
        path: Option<String>,
        /// For `sources`: list every file in a multi-file edge instead of the first 8.
        #[arg(long)]
        full: bool,
    },
    /// Read table data as CSV/TSV/NDJSON over the logical cross-block view.
    ///
    /// Extracts table data over the **logical** (cross-block) view as CSV/TSV/NDJSON — a read of
    /// `events` spans every `events_NNNN` block, projecting only the requested columns per block.
    Read {
        /// The `.tsra` to read.
        file: PathBuf,
        /// Table block, or a multi-block prefix like `events`.
        block: String,
        /// Columns to project (repeatable **or** comma-list: `-c ms -c en` or `-c ms,en`). Omit for
        /// all columns in schema order.
        #[arg(long = "column", short = 'c', value_delimiter = ',')]
        column: Vec<String>,
        /// Row range `A:B` (half-open); each side optional/negative-from-end: `91500:`, `:100`,
        /// `-10:-1`, `:`. Overrides `--limit`.
        #[arg(long, allow_hyphen_values = true, conflicts_with_all = ["head", "tail", "at"])]
        rows: Option<String>,
        /// The first N rows.
        #[arg(long, conflicts_with_all = ["rows", "tail", "at"])]
        head: Option<u64>,
        /// The last N rows.
        #[arg(long, conflicts_with_all = ["rows", "head", "at"])]
        tail: Option<u64>,
        /// Exactly the one row at index I (negative = from the end, e.g. `--at -1`).
        #[arg(long, allow_hyphen_values = true, conflicts_with_all = ["rows", "head", "tail"])]
        at: Option<i64>,
        /// Emit every row (overrides `--limit`).
        #[arg(long)]
        all: bool,
        /// Default max rows when no `--rows`/`--head`/`--tail`/`--at`/`--all` is given.
        #[arg(long, default_value_t = 20)]
        limit: u64,
        /// Output format: `csv` (default) | `tsv` | `ndjson`.
        #[arg(long, default_value = "csv")]
        format: String,
    },
    /// Numeric overview of an **array** block (shape, dtype, value range, spatial referencing).
    ///
    /// Decodes the array once and reports shape · dtype · chunks · codec · min/max/mean/std (raw,
    /// plus physical units when a rescale is present) · whether a world affine is carried.
    Stats {
        /// The `.tsra` to read.
        file: PathBuf,
        /// The array block to summarise (e.g. `volume`).
        block: String,
    },
    /// Pull a rectangular sub-region (2-D plane / 1-D line / point) of an **array** block as CSV.
    ///
    /// `--index` is numpy-style, C-order, per axis: `N` (one index, negative from end), `:` (whole
    /// axis), or `A:B` (half-open). Only intersecting chunks are decoded. Axial CT plane example:
    /// `tsra slice ct.tsra volume --index "445,:,:"`.
    Slice {
        /// The `.tsra` to read.
        file: PathBuf,
        /// The array block to slice (e.g. `volume`).
        block: String,
        /// Numpy-style per-axis index, e.g. `445,:,:` or `400:500,:,256`.
        #[arg(long, allow_hyphen_values = true, required_unless_present = "world")]
        index: Option<String>,
        /// World `L,P,S` mm point → nearest voxel via the stored affine (needs a 3-D array with a
        /// world_frame). E.g. `--world "12.3,-45.6,80.0"`. Mutually exclusive with `--index`.
        #[arg(long, allow_hyphen_values = true, conflicts_with = "index")]
        world: Option<String>,
        /// Apply the stored rescale (CT→HU, PET→Bq/mL) instead of raw stored samples.
        #[arg(long)]
        physical: bool,
        /// Output format: `csv` (default) | `tsv`.
        #[arg(long, default_value = "csv")]
        format: String,
    },
    /// Collapse an **array** block along one axis into a projection image (MIP / mean / sum).
    ///
    /// A 3-D volume → a 2-D image. `--mode max` (MIP) over the z axis is the classic PET/CT overview.
    /// `--axis` is an axis name (`z`/`y`/`x`) or index. `--physical` applies the rescale.
    Project {
        /// The `.tsra` to read.
        file: PathBuf,
        /// The array block to project (e.g. `volume`).
        block: String,
        /// Axis to collapse: a name from the array's axes (`z`/`y`/`x`) or a 0-based index.
        #[arg(long)]
        axis: String,
        /// Reduction: `max` (MIP, default) | `mean` | `sum`.
        #[arg(long, default_value = "max")]
        mode: String,
        /// Apply the stored rescale (CT→HU, PET→Bq/mL) instead of raw stored samples.
        #[arg(long)]
        physical: bool,
        /// Output format: `csv` (default) | `tsv`.
        #[arg(long, default_value = "csv")]
        format: String,
    },
    /// Build a multiscale pyramid of an array block (full-res + 2× downsampled levels) → a new `.tsra`.
    ///
    /// Emits a `recon` product with `<block>`, `<block>/1`, `<block>/2`, … (each a 2× max-downsample
    /// carrying its `at_level` affine), `derived_from` the source. Coarse levels give fast overviews.
    Pyramid {
        /// The source `.tsra`.
        file: PathBuf,
        /// The 3-D array block to build a pyramid of (e.g. `volume`).
        block: String,
        /// Output `.tsra` path for the pyramid product.
        out: PathBuf,
        /// Max downsample levels (default: until the coarsest axis ≤ 64).
        #[arg(long)]
        levels: Option<u32>,
    },
    /// Initialize a content-addressed repository for CoW versioning.
    ///
    /// Creates the repository layout (`objects/` + `refs/` + `log/`) used for copy-on-write
    /// versioning (ADR-0036).
    Init {
        /// Directory to initialize as a content-addressed repository.
        repo: PathBuf,
    },
    /// Import a sealed `.tsra` as the first version of its lineage.
    ///
    /// Imports a sealed `.tsra` into a repository as the first version of its lineage (its `id`).
    Import {
        /// The repository directory.
        repo: PathBuf,
        /// The sealed `.tsra` to import.
        file: PathBuf,
    },
    /// Commit a new version of a lineage (reuses unchanged blocks by digest).
    ///
    /// Commits a new version of a lineage with a metadata delta — reuses every unchanged block by
    /// digest, so the copy is proportional to the change (a metadata edit writes one new object).
    Commit {
        /// The repository directory.
        repo: PathBuf,
        /// The lineage id (printed by `import` / `log`).
        lineage: String,
        /// Metadata field to set, `key=value` (value parsed as JSON, else a bare string). Repeatable.
        #[arg(long = "set")]
        set: Vec<String>,
        /// Attach an already-encoded block from another `.tsra`: `[NAME=]SOURCE.tsra:BLOCK`
        /// (composition, not encoding). Repeatable.
        #[arg(long = "add-block")]
        add_block: Vec<String>,
        /// Remove a block by name (manifest edit; the object stays for other versions). Repeatable.
        #[arg(long = "remove-block")]
        remove_block: Vec<String>,
    },
    /// Show a lineage's version history (newest first).
    Log {
        /// The repository directory.
        repo: PathBuf,
        /// The lineage id (printed by `import`).
        lineage: String,
    },
    /// Diff two versions (blocks + metadata) with a lineage verdict.
    ///
    /// With one ref, diffs that version against its `supersedes` parent — "what this version
    /// changed".
    Diff {
        /// The repository directory.
        repo: PathBuf,
        /// Base version `manifest_hash` (or the target, if TARGET is omitted).
        first: String,
        /// Target version `manifest_hash` (optional).
        second: Option<String>,
    },
    /// Export a version to a standalone `.tsra` with its history (`git bundle`).
    ///
    /// The manifest is emitted as stored, supersedes + derivation edges intact — for archival
    /// where lineage travels with the data.
    Seal {
        /// The repository directory.
        repo: PathBuf,
        /// Version `manifest_hash` to export.
        version: String,
        /// Output `.tsra` path.
        out: PathBuf,
    },
    /// Export a history-free standalone `.tsra` for publication (`git archive`).
    ///
    /// Drops the supersedes chain, keeps derivation provenance + a `snapshot_of` breadcrumb —
    /// for publication / handover.
    Publish {
        /// The repository directory.
        repo: PathBuf,
        /// Version `manifest_hash` to publish.
        version: String,
        /// Output `.tsra` path.
        out: PathBuf,
        /// Drop even the single `snapshot_of` back-pointer (blind / clinical handover).
        #[arg(long)]
        anonymous: bool,
    },
    /// Forget a lineage (deletes its ref + log; objects reclaimed by `gc`).
    ///
    /// Now-unreachable objects are reclaimed by `gc` (blocks shared with other lineages stay).
    Forget {
        /// The repository directory.
        repo: PathBuf,
        /// The lineage id to forget.
        lineage: String,
    },
    /// Reclaim objects unreachable from any ref (run after `forget`).
    Gc {
        /// The repository directory.
        repo: PathBuf,
    },
    /// Push a sealed `.tsra` to an OCI registry (needs `--features cloud`).
    ///
    /// Uses the in-Rust OCI distribution client; needs the `cloud` feature built in.
    Push {
        /// The sealed `.tsra` to push.
        file: PathBuf,
        /// Registry reference: `[oci://]host[:port]/repo:tag`.
        reference: String,
        /// Use plain HTTP (self-hosted / CI registries) instead of HTTPS.
        #[arg(long)]
        plain_http: bool,
        /// Basic-auth username (password via `--password`).
        #[arg(long)]
        username: Option<String>,
        /// Basic-auth password.
        #[arg(long)]
        password: Option<String>,
    },
    /// Pull a `.tsra` OCI artifact from a registry (needs `--features cloud`).
    ///
    /// sha256-verified; needs the `cloud` feature built in.
    Pull {
        /// Registry reference: `[oci://]host[:port]/repo:tag`.
        reference: String,
        /// Output `.tsra` path.
        out: PathBuf,
        /// Use plain HTTP instead of HTTPS.
        #[arg(long)]
        plain_http: bool,
        /// Basic-auth username.
        #[arg(long)]
        username: Option<String>,
        /// Basic-auth password.
        #[arg(long)]
        password: Option<String>,
    },
    /// Extract one block's raw bytes to a file (digest-verified on read).
    ///
    /// Recovers a `blob` (preserved file) **byte-identical**, or any block's stored payload. The
    /// digest is re-checked on read.
    Extract {
        /// The `.tsra` to read.
        file: PathBuf,
        /// Block name (e.g. `data` for a blob product).
        block: String,
        /// Output path for the recovered bytes.
        out: PathBuf,
    },
    /// Explode a `.tsra` into a directory (`manifest.json` + `blocks/<name>`).
    Unpack {
        /// The `.tsra` to explode.
        file: PathBuf,
        /// Output directory to write `manifest.json` + `blocks/<name>`.
        outdir: PathBuf,
    },
    /// Pack an exploded directory (`manifest.json` + `blocks/`) into a sealed `.tsra`.
    Pack {
        /// Exploded directory containing `manifest.json` + `blocks/`.
        dir: PathBuf,
        /// Output `.tsra` path.
        out: PathBuf,
    },
    /// Validate a `.tsra`'s manifest against its declared product schema (required fields/blocks).
    Schema {
        /// The `.tsra` to validate.
        file: PathBuf,
        /// Print the embedded product schema as JSON (the file's own contract) instead of validating.
        #[arg(long)]
        json: bool,
    },
    /// Ingest a vendor file into a sealed `.tsra` (or run a declarative `--spec`).
    ///
    /// Ingests a vendor acquisition file into a sealed `.tsra` product (normalise at the door),
    /// or runs a declarative ingest spec (`--spec FILE`) into a sealed collection of `.tsra`
    /// products.
    Ingest {
        /// Path to a `.toml` ingest spec (ADR-0035). When given, the per-format subcommand is not
        /// required — the spec describes the whole multi-product acquisition + its derivation DAG.
        /// (Clap doesn't allow `conflicts_with` against a subcommand; the runtime dispatcher
        /// rejects the combination explicitly with a clear message.)
        #[arg(long)]
        spec: Option<PathBuf>,
        /// Output directory for the spec engine — one `.tsra` per member + `collection.json`.
        /// Defaults to `./ingest-out`.
        #[arg(long, requires = "spec")]
        out: Option<PathBuf>,
        /// Encode-thread pool for the streaming write engine (default: available parallelism).
        /// Runtime knob — never part of the spec (specs must be machine-portable).
        #[arg(long, requires = "spec")]
        workers: Option<usize>,
        /// RAM ceiling for the in-flight encode ring (accepts `512M` / `1G` / `8GiB`). Runtime
        /// knob — never part of the spec.
        #[arg(long, requires = "spec")]
        ram_budget: Option<String>,
        /// Warmup-measure read+per-core encode rate, then ask the `balanced` heuristic for the
        /// worker knee. Runtime knob — never part of the spec.
        #[arg(long, requires = "spec", conflicts_with = "workers")]
        auto: bool,
        /// Byte threshold above which `hdf-compound` with `streaming = "auto"` switches to the
        /// streaming path. Default: 256 MiB.
        #[arg(long, requires = "spec")]
        stream_threshold: Option<String>,
        #[command(subcommand)]
        src: Option<IngestSrc>,
    },
    /// Emit a FAIR discovery record for a `.tsra` (prints JSON to stdout).
    Export {
        /// The `.tsra` to export.
        file: PathBuf,
        /// Record format: `ro-crate` (default) | `datacite`.
        #[arg(long, default_value = "ro-crate")]
        format: String,
    },
    /// Generate an ed25519 keypair (private seed mode 0600; public key to `OUT.pub`).
    ///
    /// Writes the private seed (mode 0600) to OUT and the public key to `OUT.pub`.
    Keygen {
        /// Output path for the private key seed (the public key is written to `OUT.pub`).
        out: PathBuf,
        /// Generate an `age` X25519 keypair for crypto-shred de-identification (ADR-0047) instead of
        /// the default ed25519 **signing** key. The secret goes to OUT, the recipient to `OUT.pub`.
        #[arg(long)]
        age: bool,
    },
    /// Manage the trust store of public keys `verify-sig` accepts.
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
    /// Sign a sealed `.tsra` (embeds an `aux/signatures/<key_id>.sig.json` member by default).
    ///
    /// ed25519 over the signature envelope; the result rides **inside** the container as a
    /// non-sealed aux member (ADR-0042), so a `.tsra` shared by itself never loses its signature.
    /// Pass `--sidecar` to write the legacy detached `<file>.tsra.sig.json` instead.
    Sign {
        /// The sealed `.tsra` to sign.
        file: PathBuf,
        /// Hex-encoded 32-byte ed25519 signing-key (seed) file (from `tessera keygen`).
        #[arg(long)]
        key: PathBuf,
        /// Optional signer identity recorded in the signature (e.g. an ORCID iD URL).
        #[arg(long)]
        signer: Option<String>,
        /// Write the legacy detached `<file>.tsra.sig.json` sidecar instead of embedding the
        /// signature. Kept for the OCI double-layer distribution path (ADR-0037 §0 bug #2).
        #[arg(long)]
        sidecar: bool,
    },
    /// Verify a sealed `.tsra` against its signature (embedded first, `<file>.tsra.sig.json` fallback).
    ///
    /// Defaults to the **trust store** (the signature's `key_id` must be a key you trust);
    /// `--pubkey` checks against an explicit key.
    VerifySig {
        /// The sealed `.tsra` to verify.
        file: PathBuf,
        /// Explicit hex ed25519 public-key file (skips the trust store).
        #[arg(long)]
        pubkey: Option<PathBuf>,
        /// Also require the signature's `signer` to equal this identity (e.g. an ORCID iD URL).
        #[arg(long)]
        require_signer: Option<String>,
    },
    /// Recover a crypto-shred product's identity with a recipient private key (ADR-0047).
    ///
    /// Decrypts the `aux/identity/identity.age` envelope carried inside a `--crypto-shred` product
    /// and prints the recovered identity document (full DICOM header + curated identifying fields).
    /// The sealed product itself is de-identified; this is the "un-SOP" that a key holder can do.
    Reidentify {
        /// The crypto-shred `.tsra` to re-identify.
        file: PathBuf,
        /// Recipient `age` identity (private key) file — the `AGE-SECRET-KEY-1…` produced alongside
        /// the recipient public key given to `ingest --recipient`.
        #[arg(long)]
        identity: PathBuf,
        /// Write the recovered identity JSON here instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Crypto-shred: permanently remove a product's identity envelope (ADR-0047).
    ///
    /// Drops `aux/identity/` from the `.tsra` in place. The seal is untouched (the envelope rides
    /// outside it), so the file still verifies — the identifying material is simply gone from this
    /// copy. Combined with destroying the recipient private key, it is gone everywhere.
    Shred {
        /// The `.tsra` whose identity envelope to remove.
        file: PathBuf,
    },
    /// Run SQL (DataFusion) over a `.tsra` table block (needs `--features sql`).
    ///
    /// Registers the block's [`LogicalTableView`] as an Arrow `MemTable` under the block name
    /// (so `FROM events` transparently spans every `events_NNNN` shard) and hands the query to
    /// DataFusion. Result is CSV (default) / TSV.
    ///
    /// Spike phase (#251): the whole block is materialized in RAM before the query runs —
    /// streaming + predicate/projection pushdown is #251 phase 2/3. `tessera sql` still parses
    /// without the feature and returns a clear "rebuild with --features sql" error.
    Sql {
        /// The `.tsra` to query.
        file: PathBuf,
        /// Table block (or multi-block prefix like `events`) to expose as the SQL table.
        block: String,
        /// The SQL query (`FROM <block>`; SELECT/WHERE/ORDER BY/LIMIT are the phase-1 target).
        query: String,
        /// Output format: `csv` (default) | `tsv`. `ndjson` is a follow-up.
        #[arg(long, default_value = "csv")]
        format: String,
    },
    /// Bench the write engine on this host (throughput + peak RSS).
    ///
    /// Drives the real `StreamWriter`/`TableStreamWriter` and reports throughput + peak RSS so an
    /// operator can size RAM/threads for their acquisition rate.
    Bench {
        #[command(subcommand)]
        action: BenchAction,
    },
}

#[derive(Subcommand)]
enum BenchAction {
    /// Bench the streaming write engine (events/s + MB/s + peak RAM).
    ///
    /// Drives synthetic data or a real `.h5` through the streaming write engine.
    Write {
        /// Schema to drive: `listmode` (single-thread encode) or `blocks` (parallelizes).
        #[arg(long, default_value = "listmode")]
        schema: String,
        /// Synthetic event/block count (ignored when --input is given). ~1M is the throughput.rs default.
        #[arg(long, default_value_t = 1_048_576)]
        rows: usize,
        /// RAM ceiling for the in-flight encode ring (accepts `512M` / `1G` / `8GiB`). Defaults to 1 GiB.
        #[arg(long)]
        ram_budget: Option<String>,
        /// Encode-thread pool (single run). Mutually advisory with `--sweep`/`--auto`; without
        /// any of them, defaults to `available_parallelism()`.
        #[arg(long, conflicts_with_all = ["sweep", "auto"])]
        workers: Option<usize>,
        /// Sweep workers 1,2,4,8,… up to `available_parallelism` and print a table — the
        /// "size your system" mode.
        #[arg(long, conflicts_with_all = ["workers", "auto"])]
        sweep: bool,
        /// Warmup-measure read+transpose vs per-core encode rate, ask the `balanced` heuristic
        /// (ADR-0034) for the worker knee, then run ONCE at that recommendation. The
        /// "adaptive thread allocator" mode — picks the knee, not max-cores.
        #[arg(long, conflicts_with_all = ["workers", "sweep"])]
        auto: bool,
        /// Real listmode `.h5` to ingest (overrides synthetic data). Drives the production
        /// `stream_to_listmode_product_2p` path.
        #[arg(long)]
        input: Option<PathBuf>,
        /// HDF5 dataset name to read from `--input` (default: `events_2p`).
        #[arg(long, default_value = "events_2p")]
        dataset: String,
        /// Seed for the MC sampler (deterministic synthetic data).
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
}

#[derive(Subcommand)]
enum TrustAction {
    /// Trust a public key under a handle.
    ///
    /// Stores it in the user store, or `--repo` for `.tessera/trust/`.
    Add {
        /// Hex ed25519 public-key file (e.g. `key.pub` from `tessera keygen`).
        pubkey: PathBuf,
        /// A handle for this key.
        #[arg(long)]
        name: String,
        /// The signer identity to record alongside it (e.g. an ORCID iD URL).
        #[arg(long)]
        signer: Option<String>,
        /// Store in the repo-local `.tessera/trust/` instead of the user store.
        #[arg(long)]
        repo: bool,
    },
    /// List trusted keys.
    List,
    /// Remove a trusted key by handle or `key_id`.
    Remove {
        /// Handle or `key_id` of the key to remove.
        target: String,
    },
}

#[derive(Subcommand)]
enum IngestSrc {
    /// DICOM image → `recon` product (lossless int16 + provenance).
    ///
    /// Encodes a DICOM image/series to a `recon` product with lossless int16 + rescale/units/
    /// modality + provenance.
    Dicom {
        /// Source `.dcm` file.
        input: PathBuf,
        /// Output `.tsra`.
        out: PathBuf,
        /// Product name (the human handle in the manifest).
        #[arg(long)]
        name: String,
        /// Acquisition timestamp (ISO-8601), recorded verbatim in the manifest.
        #[arg(long)]
        timestamp: String,
        /// Apply PS3.15 de-identification (drop PHI tags) before encoding.
        #[arg(long)]
        deidentify: bool,
        /// Crypto-shred de-identification (ADR-0047): de-identify AND encrypt the stripped identity to
        /// this `age` recipient public key (`age1…`), carried as an `aux/identity` envelope inside the
        /// `.tsra`. Repeatable for multiple recipients. `tessera reidentify` recovers it with the
        /// matching private key; `tessera shred` (or destroying the key) makes it irrecoverable.
        #[arg(long = "recipient", value_name = "AGE_PUBKEY")]
        recipient: Vec<String>,
        /// Clean label for the `ingested_from` provenance edge — replaces the input PATH in the
        /// sealed manifest (ADR-0040 PHI hygiene: an absolute path on clinical data is itself PHI).
        #[arg(long, value_name = "LABEL")]
        source_label: Option<String>,
        /// Attach metadata `key=value` (value parsed as JSON, else string) — supplies/overrides schema
        /// fields. Repeatable: `--meta study=DUPLET-07 --meta modality=PT`.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
    },
    /// DICOM series (multi-file slice stack) → 3-D `recon` product.
    ///
    /// Stacks a multi-file CT/PET slice series into one 3-D `recon` product.
    DicomSeries {
        /// The `.dcm` slice files of the series (uniform shape/modality/rescale; else rejected).
        inputs: Vec<PathBuf>,
        /// Output `.tsra`.
        #[arg(long)]
        out: PathBuf,
        /// Product name (the human handle in the manifest).
        #[arg(long)]
        name: String,
        /// Acquisition timestamp (ISO-8601).
        #[arg(long)]
        timestamp: String,
        /// Apply PS3.15 de-identification per-slice (strip PHI tags from each `.dcm` in memory before
        /// the volume is stacked; source files are NOT mutated).
        #[arg(long)]
        deidentify: bool,
        /// Crypto-shred de-identification (ADR-0047): de-identify AND encrypt the stripped identity to
        /// this `age` recipient public key, carried as an `aux/identity` envelope. Repeatable. See
        /// `tessera reidentify` / `tessera shred`.
        #[arg(long = "recipient", value_name = "AGE_PUBKEY")]
        recipient: Vec<String>,
        /// Clean label for the `ingested_from` provenance edge — replaces the per-slice joined paths
        /// (an N-slice series would embed N PHI-bearing absolute paths). ADR-0040.
        #[arg(long, value_name = "LABEL")]
        source_label: Option<String>,
        /// Attach metadata `key=value` (value parsed as JSON, else string). Repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
    },
    /// GE listmode HDF5 → `listmode` product (compound → columnar).
    ///
    /// Transposes compound events into columnar form (the #193 transpose).
    GeHdf5 {
        /// Source `.h5` file.
        input: PathBuf,
        /// Output `.tsra`.
        out: PathBuf,
        /// Product name.
        #[arg(long)]
        name: String,
        /// Acquisition timestamp (ISO-8601).
        #[arg(long)]
        timestamp: String,
        /// Compound dataset to read: `events_3p` (default) or `events_2p`.
        #[arg(long, default_value = "events_3p")]
        dataset: String,
        /// Clean label for the `ingested_from` provenance edge — replaces the input PATH in the
        /// sealed manifest (ADR-0040 PHI hygiene).
        #[arg(long, value_name = "LABEL")]
        source_label: Option<String>,
        /// Attach metadata `key=value` (value parsed as JSON, else string). Repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
    },
    /// Preserve an un-parsed file as an opaque `blob` product (the "junk" tier).
    ///
    /// Preserves an un-parsed file **bit-faithfully** as an opaque `blob` (`.l64`, `.7z`, PDF;
    /// bytes stored verbatim, blake3-sealed). Aka `junk`, for when the vendor file has earned the
    /// name.
    #[command(alias = "junk")]
    Blob {
        /// Source file (anything — the engine does not parse it).
        input: PathBuf,
        /// Output `.tsra`.
        out: PathBuf,
        /// Product name.
        #[arg(long)]
        name: String,
        /// Acquisition timestamp (ISO-8601).
        #[arg(long)]
        timestamp: String,
        /// IANA media type, if known (e.g. `application/pdf`). Defaults to opaque octet-stream.
        #[arg(long)]
        media_type: Option<String>,
        /// Clean label for the `ingested_from` provenance edge — replaces the input PATH in the
        /// sealed manifest (ADR-0040 PHI hygiene; vendor `.l64` filenames often carry patient ids).
        #[arg(long, value_name = "LABEL")]
        source_label: Option<String>,
        /// Attach metadata `key=value` (value parsed as JSON, else string) — e.g. `--meta study=DUPLET-07`
        /// to satisfy the blob schema's recommended `study`. Repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
    },
}

fn main() -> ExitCode {
    // Surface library WARNs (e.g. the ingest engine's recommended-field nudge) on stderr. Quiet
    // (warn+ only), compact + timestamp-free so it reads as CLI output rather than a log.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
    match run(Cli::parse().cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cmd: Cmd) -> tessera_core::Result<()> {
    match cmd {
        Cmd::Inspect { file, full } => {
            let r = open_local_or_url(&file)?;
            let m = r.manifest();
            println!("tessera {} · product={}", m.tessera_version, m.product);
            println!("id            {}", m.id);
            println!("name          {}", m.name);
            println!("timestamp     {}", m.timestamp);
            if let Some(p) = &m.producer {
                println!("producer      {p}");
            }
            if let Some(s) = &m.study {
                println!("study         {s}");
            }
            println!("content_hash  {}", m.content_hash.as_deref().unwrap_or("-"));
            println!(
                "manifest_hash {}",
                m.manifest_hash.as_deref().unwrap_or("-")
            );
            println!("blocks        {}", m.blocks.len());
            for b in &m.blocks {
                println!(
                    "  - {:<20} {:<6?} {}",
                    b.name,
                    b.kind,
                    b.digest.as_deref().unwrap_or("-")
                );
            }
            if !m.sources.is_empty() {
                println!("sources       {}", m.sources.len());
                for s in &m.sources {
                    // The edge's `content_hash` is the integrity link (source merkle root for
                    // `ingested_from`; parent `manifest_hash` / spec_hash for derived/spec edges).
                    let integrity = s
                        .content_hash
                        .as_deref()
                        .map(|h| format!("  [{}]", nav::short_hash(h)))
                        .unwrap_or_default();
                    println!(
                        "  - {} <- {}{integrity}",
                        s.role,
                        nav::compact_reference(&s.reference, full)
                    );
                }
            }
            Ok(())
        }
        Cmd::Verify { file } => {
            let mut r = open_local_or_url(&file)?; // magic + manifest seal
            let n = r.manifest().blocks.len();
            for name in r.block_names() {
                r.read_block_by_name(&name)?; // payload bytes vs recorded digest
            }
            println!("OK  {} verified ({n} blocks)", file.display());
            Ok(())
        }
        Cmd::Reidentify {
            file,
            identity,
            out,
        } => {
            // The recipient private key (`AGE-SECRET-KEY-1…`), read from disk and never logged.
            let key = std::fs::read_to_string(&identity)?;
            let mut r = Reader::open(&file)?;
            let envelope = r
                .read_aux(tessera_ingest::identity::AUX_IDENTITY_NAME)
                .map_err(|_| {
                    tessera_core::Error::Invalid(format!(
                        "{} carries no crypto-shred identity envelope (aux/{}) — was it ingested with --recipient?",
                        file.display(),
                        tessera_ingest::identity::AUX_IDENTITY_NAME
                    ))
                })?;
            let doc = tessera_ingest::identity::decrypt_identity(&envelope, &key)?;
            let json = serde_json::to_string_pretty(&doc)?;
            match out {
                Some(p) => {
                    std::fs::write(&p, json)?;
                    println!("recovered identity -> {}", p.display());
                }
                None => println!("{json}"),
            }
            Ok(())
        }
        Cmd::Shred { file } => {
            // Remove aux/identity/* in place; the seal (id/content_hash/manifest_hash) is untouched.
            tessera_io::write_aux_members(&file, &[], &["identity/"])?;
            println!("shredded identity envelope from {}", file.display());
            Ok(())
        }
        Cmd::Tree { file, full } => {
            let mut out = std::io::stdout().lock();
            nav::tree(&file, full, &mut out)
        }
        Cmd::Ls { file, path, full } => {
            let mut out = std::io::stdout().lock();
            nav::ls(&file, path.as_deref(), full, &mut out)
        }
        Cmd::Read {
            file,
            block,
            column,
            rows,
            head,
            tail,
            at,
            all,
            limit,
            format,
        } => {
            let fmt = nav::Format::parse(&format)?;
            // clap enforces mutual exclusion; map whichever was given to a RowSpec.
            let rows = match (rows, head, tail, at) {
                (Some(s), ..) => Some(nav::RowSpec::parse_range(&s)?),
                (_, Some(n), ..) => Some(nav::RowSpec::Head(n)),
                (_, _, Some(n), _) => Some(nav::RowSpec::Tail(n)),
                (_, _, _, Some(i)) => Some(nav::RowSpec::At(i)),
                _ => None,
            };
            let mut out = std::io::stdout().lock();
            let res = nav::read(
                nav::ReadOpts {
                    file: &file,
                    block: &block,
                    columns: column,
                    rows,
                    all,
                    limit,
                    format: fmt,
                },
                &mut out,
            )?;
            if res.truncated {
                eprintln!(
                    "note: showed {} of {} rows — pass --all or --rows A:B for the rest",
                    res.shown, res.total
                );
            }
            Ok(())
        }
        Cmd::Stats { file, block } => {
            let mut out = std::io::stdout().lock();
            nav::stats(&file, &block, &mut out)
        }
        Cmd::Slice {
            file,
            block,
            index,
            world,
            physical,
            format,
        } => {
            let fmt = nav::Format::parse(&format)?;
            let mut out = std::io::stdout().lock();
            nav::slice(
                &file,
                &block,
                index.as_deref(),
                world.as_deref(),
                physical,
                fmt,
                &mut out,
            )
        }
        Cmd::Project {
            file,
            block,
            axis,
            mode,
            physical,
            format,
        } => {
            let fmt = nav::Format::parse(&format)?;
            let mut out = std::io::stdout().lock();
            nav::project(&file, &block, &axis, &mode, physical, fmt, &mut out)
        }
        Cmd::Pyramid {
            file,
            block,
            out,
            levels,
        } => {
            let n = nav::build_pyramid(&file, &block, levels, &out)?;
            println!("wrote {} ({n} levels)", out.display());
            Ok(())
        }
        Cmd::Init { repo } => {
            let mut out = std::io::stdout().lock();
            version::init(&repo, &mut out)
        }
        Cmd::Import { repo, file } => {
            let mut out = std::io::stdout().lock();
            version::import(&repo, &file, &mut out)
        }
        Cmd::Commit {
            repo,
            lineage,
            set,
            add_block,
            remove_block,
        } => {
            let mut out = std::io::stdout().lock();
            version::commit(&repo, &lineage, &set, &add_block, &remove_block, &mut out)
        }
        Cmd::Log { repo, lineage } => {
            let mut out = std::io::stdout().lock();
            version::log(&repo, &lineage, &mut out)
        }
        Cmd::Diff {
            repo,
            first,
            second,
        } => {
            let mut out = std::io::stdout().lock();
            version::diff(&repo, &first, second.as_deref(), &mut out)
        }
        Cmd::Seal { repo, version, out } => {
            let mut w = std::io::stdout().lock();
            version::seal(&repo, &version, &out, &mut w)
        }
        Cmd::Publish {
            repo,
            version,
            out,
            anonymous,
        } => {
            let mut w = std::io::stdout().lock();
            version::publish(&repo, &version, &out, anonymous, &mut w)
        }
        Cmd::Forget { repo, lineage } => {
            let mut w = std::io::stdout().lock();
            version::forget(&repo, &lineage, &mut w)
        }
        Cmd::Gc { repo } => {
            let mut w = std::io::stdout().lock();
            version::gc(&repo, &mut w)
        }
        Cmd::Push {
            file,
            reference,
            plain_http,
            username,
            password,
        } => do_push(&file, &reference, plain_http, username, password),
        Cmd::Pull {
            reference,
            out,
            plain_http,
            username,
            password,
        } => do_pull(&reference, &out, plain_http, username, password),
        Cmd::Unpack { file, outdir } => {
            let m = unpack(&file, &outdir)?;
            println!(
                "unpacked {} ({} blocks) -> {}",
                m.id,
                m.blocks.len(),
                outdir.display()
            );
            Ok(())
        }
        Cmd::Pack { dir, out } => {
            pack_dir(&dir, &out)?;
            println!("packed {} -> {}", dir.display(), out.display());
            Ok(())
        }
        Cmd::Extract { file, block, out } => {
            // Stream the block in bounded memory (no whole-block Vec) — a multi-GB blob extracts
            // without buffering, and over a cloud source the zip read range-GETs just it. Stage to a
            // sibling `.part` and atomically rename only AFTER the digest verifies, so a corrupt
            // block never leaves unverified bytes at the destination path.
            //
            // Routed through `open_local_or_url` so `s3://…` / `http(s)://…` work transparently when
            // the `cloud` feature is built; without it, it's a thin wrapper over `Reader::open` so
            // local extract behaves exactly as before.
            let mut r = open_local_or_url(&file)?;
            let mut tmp = out.clone().into_os_string();
            tmp.push(".part");
            let tmp = PathBuf::from(tmp);
            let n = {
                let f = std::fs::File::create(&tmp).map_err(tessera_core::Error::from)?;
                let mut w = std::io::BufWriter::new(f);
                match r.stream_block_to(&block, &mut w).and_then(|n| {
                    std::io::Write::flush(&mut w)
                        .map(|()| n)
                        .map_err(Into::into)
                }) {
                    Ok(n) => n,
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(e);
                    }
                }
            };
            std::fs::rename(&tmp, &out).map_err(tessera_core::Error::from)?;
            println!("extracted {block} ({n} bytes) -> {}", out.display());
            Ok(())
        }
        Cmd::Schema { file, json } => {
            let r = Reader::open(&file)?;
            let m = r.manifest();
            // Prefer the schema **embedded in the file** (self-describing) over the binary's
            // registry — a sealed `.tsra` carries its own contract (obligatory since the
            // self-describing batch). `--json` dumps that embedded contract verbatim.
            let embedded = m
                .schema
                .as_ref()
                .map(tessera_core::ProductSchema::from_value)
                .transpose()?;
            if json {
                let schema = match &embedded {
                    Some(s) => s.to_value()?,
                    None => SchemaRegistry::builtin()
                        .get(&m.product)
                        .ok_or_else(|| {
                            tessera_core::Error::Invalid(format!(
                                "no schema for open-world product '{}'",
                                m.product
                            ))
                        })?
                        .to_value()?,
                };
                println!("{}", serde_json::to_string_pretty(&schema)?);
                return Ok(());
            }
            match (&embedded, SchemaRegistry::builtin().get(&m.product)) {
                (Some(s), reg) => {
                    let drift = reg
                        .filter(|r| r.version != s.version)
                        .map(|r| format!(" (registry has v{})", r.version))
                        .unwrap_or_default();
                    println!(
                        "product '{}' — embedded schema v{} ({}){drift}",
                        m.product, s.version, s.description
                    );
                }
                (None, Some(s)) => println!(
                    "product '{}' — schema v{} ({}) from the built-in registry (not embedded in this file)",
                    m.product, s.version, s.description
                ),
                (None, None) => println!(
                    "product '{}' — unknown schema (open-world: validation is permissive)",
                    m.product
                ),
            }
            // Field roster: what the schema *declares* vs what this product *carries* — so a user
            // sees the available fields (populated + missing) with their tier + PHI sensitivity,
            // not only whatever happens to be filled in. (Answers "always show the avail fields".)
            let schema = embedded
                .clone()
                .or_else(|| SchemaRegistry::builtin().get(&m.product).cloned());
            if let Some(s) = schema.filter(|s| !s.fields.is_empty()) {
                println!("fields ({} declared):", s.fields.len());
                for f in &s.fields {
                    let (mark, tier) = if f.required {
                        ('●', "required")
                    } else if f.recommended {
                        ('◐', "recommended")
                    } else {
                        ('·', "optional")
                    };
                    let sens = format!("{:?}", f.sensitivity).to_lowercase();
                    let val = match m.metadata.get(&f.id) {
                        Some(v) => format!("= {}", compact_json(v)),
                        None => match &f.default {
                            Some(d) => format!("— (default {})", compact_json(d)),
                            None => "—".to_string(),
                        },
                    };
                    println!("  {mark} {:<22} {tier:<12} {sens:<11} {val}", f.id);
                }
            }
            tessera_core::validate_manifest(m)?; // typed error naming the first missing field/block
            println!("OK  schema-valid: {}", file.display());
            Ok(())
        }
        Cmd::Ingest {
            spec,
            out,
            workers,
            ram_budget,
            auto,
            stream_threshold,
            src,
        } => {
            // `--spec` and a per-format subcommand are mutually exclusive — both routes build a
            // 1+-product `IngestSpec` and run it through the engine, so picking both would be
            // ambiguous. Clap can't express conflict-with-a-subcommand, so we enforce it here.
            if spec.is_some() && src.is_some() {
                return Err(tessera_core::Error::Invalid(
                    "tessera ingest: --spec is mutually exclusive with the per-format \
                     subcommand (dicom / ge-hdf5); pass one or the other"
                        .into(),
                ));
            }
            if let Some(spec_path) = spec {
                run_ingest_spec(IngestSpecOpts {
                    spec_path,
                    out_dir: out,
                    workers,
                    ram_budget,
                    auto,
                    stream_threshold,
                })
            } else {
                let src = src.ok_or_else(|| {
                    tessera_core::Error::Invalid(
                        "tessera ingest: pass --spec FILE or a per-format subcommand (e.g. \
                         dicom / ge-hdf5)"
                            .into(),
                    )
                })?;
                run_ingest(src)
            }
        }
        Cmd::Export { file, format } => {
            let m = Reader::open(&file)?;
            let record = match format.as_str() {
                "ro-crate" | "rocrate" => tessera_core::export::ro_crate(m.manifest()),
                "datacite" => tessera_core::export::datacite(m.manifest()),
                other => {
                    return Err(tessera_core::Error::Invalid(format!(
                        "unknown --format '{other}' (expected ro-crate | datacite)"
                    )))
                }
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&record).map_err(tessera_core::Error::from)?
            );
            Ok(())
        }
        Cmd::Sign {
            file,
            key,
            signer,
            sidecar,
        } => {
            // Auto-detect raw-hex (keygen) vs an OpenSSH `~/.ssh/id_ed25519`; stamp the signing time.
            let (sk, key_format) = trust::load_signing_key(&key)?;
            let opts = tessera_core::signing::SignOpts {
                signer,
                signed_at: Some(trust::now_rfc3339()),
                key_format: Some(key_format.into()),
            };
            let env = if sidecar {
                tessera_io::sign_tsra_sidecar(&file, &sk, &opts)?
            } else {
                tessera_io::sign_tsra(&file, &sk, &opts)?
            };
            println!("OK  signed {}", file.display());
            if sidecar {
                println!(
                    "  sidecar   {}",
                    tessera_io::sign::sidecar_path(&file).display()
                );
            } else {
                // ADR-0042: the signature is inside the container as `aux/signatures/<key_id>.sig.json`.
                println!("  embedded  aux/signatures/{}.sig.json", env.key_id);
            }
            println!("  key_id    {}", env.key_id);
            if let Some(fmt) = &env.key_format {
                println!("  key_fmt   {fmt}");
            }
            if let Some(at) = &env.signed_at {
                println!("  signed_at {at}");
            }
            if let Some(s) = &env.signer {
                println!("  signer    {s}");
            }
            Ok(())
        }
        Cmd::Keygen { out, age } => {
            let mut w = std::io::stdout().lock();
            if age {
                trust::keygen_age(&out, &mut w)
            } else {
                trust::keygen(&out, &mut w)
            }
        }
        Cmd::Trust { action } => {
            let mut w = std::io::stdout().lock();
            let dirs = trust::trust_dirs();
            match action {
                TrustAction::Add {
                    pubkey,
                    name,
                    signer,
                    repo,
                } => {
                    let dir = if repo {
                        PathBuf::from(".tessera/trust")
                    } else {
                        trust::trust_dirs().into_iter().nth(1).ok_or_else(|| {
                            tessera_core::Error::Invalid(
                                "no user config dir ($HOME/$XDG_CONFIG_HOME unset) — pass --repo \
                                 to store in .tessera/trust"
                                    .into(),
                            )
                        })?
                    };
                    trust::add(&dir, &pubkey, &name, signer.as_deref(), &mut w)
                }
                TrustAction::List => trust::list(&dirs, &mut w),
                TrustAction::Remove { target } => trust::remove(&dirs, &target, &mut w),
            }
        }
        Cmd::VerifySig {
            file,
            pubkey,
            require_signer,
        } => {
            let mut w = std::io::stdout().lock();
            trust::verify_sig(
                &file,
                pubkey.as_deref(),
                require_signer.as_deref(),
                &trust::trust_dirs(),
                &mut w,
            )
        }
        Cmd::Sql {
            file,
            block,
            query,
            format,
        } => do_sql(&file, &block, &query, &format),
        Cmd::Bench { action } => match action {
            BenchAction::Write {
                schema,
                rows,
                ram_budget,
                workers,
                sweep,
                auto,
                input,
                dataset,
                seed,
            } => bench::run(bench::BenchOpts {
                schema,
                rows,
                ram_budget,
                workers,
                sweep,
                auto,
                input,
                dataset,
                seed,
            }),
        },
    }
}

/// Parse repeatable `--meta key=value` into the product's metadata map (the config-side supply that
/// satisfies a schema's required/recommended fields). Each `value` is parsed as JSON, falling back to a
/// bare string — same convention as `commit --set`. Shared by every `tessera ingest` subcommand so
/// metadata is supplied one generic way, not per-backend flags.
fn parse_meta(
    items: &[String],
) -> tessera_core::Result<std::collections::BTreeMap<String, serde_json::Value>> {
    let mut out = std::collections::BTreeMap::new();
    for kv in items {
        let (k, v) = kv.split_once('=').ok_or_else(|| {
            tessera_core::Error::Invalid(format!("--meta expects key=value, got '{kv}'"))
        })?;
        let value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.into()));
        out.insert(k.to_string(), value);
    }
    Ok(out)
}

/// Compact one-line JSON render of a metadata value for the `schema` field roster (truncated).
fn compact_json(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.chars().count() > 48 {
        let head: String = s.chars().take(45).collect();
        format!("{head}…")
    } else {
        s
    }
}

/// `tessera push` — with the `cloud` feature, runs the in-Rust OCI distribution client; without it,
/// a clear "rebuild with --features cloud" error (the registry transport pulls reqwest).
#[cfg(feature = "cloud")]
fn do_push(
    file: &std::path::Path,
    reference: &str,
    plain_http: bool,
    username: Option<String>,
    password: Option<String>,
) -> tessera_core::Result<()> {
    let auth = username.map(|u| (u, password.unwrap_or_default()));
    let pushed = tessera_io::registry_push(file, reference, plain_http, auth)?;
    println!("pushed {} -> {pushed}", file.display());
    Ok(())
}

#[cfg(not(feature = "cloud"))]
fn do_push(
    _file: &std::path::Path,
    _reference: &str,
    _plain_http: bool,
    _username: Option<String>,
    _password: Option<String>,
) -> tessera_core::Result<()> {
    Err(tessera_core::Error::Invalid(
        "push requires the `cloud` feature — rebuild: cargo build -p tessera-cli --features cloud"
            .into(),
    ))
}

/// `tessera pull` — mirror of [`do_push`] for the pull side.
#[cfg(feature = "cloud")]
fn do_pull(
    reference: &str,
    out: &std::path::Path,
    plain_http: bool,
    username: Option<String>,
    password: Option<String>,
) -> tessera_core::Result<()> {
    let auth = username.map(|u| (u, password.unwrap_or_default()));
    tessera_io::registry_pull(reference, out, plain_http, auth)?;
    println!("pulled {reference} -> {}", out.display());
    Ok(())
}

#[cfg(not(feature = "cloud"))]
fn do_pull(
    _reference: &str,
    _out: &std::path::Path,
    _plain_http: bool,
    _username: Option<String>,
    _password: Option<String>,
) -> tessera_core::Result<()> {
    Err(tessera_core::Error::Invalid(
        "pull requires the `cloud` feature — rebuild: cargo build -p tessera-cli --features cloud"
            .into(),
    ))
}

/// `tessera sql` — with the `sql` feature, dispatches through [`crate::sql::run`] (DataFusion
/// over the block's `LogicalTableView`). Without the feature, a typed fallback error mirroring
/// `do_push` / `do_pull` — the subcommand still parses, so users get the same "rebuild with
/// --features" affordance the cloud verbs already do.
#[cfg(feature = "sql")]
fn do_sql(
    file: &std::path::Path,
    block: &str,
    query: &str,
    format: &str,
) -> tessera_core::Result<()> {
    let fmt = nav::Format::parse(format)?;
    sql::run(file, block, query, fmt)
}

#[cfg(not(feature = "sql"))]
fn do_sql(
    _file: &std::path::Path,
    _block: &str,
    _query: &str,
    _format: &str,
) -> tessera_core::Result<()> {
    Err(tessera_core::Error::Invalid(
        "sql requires the `sql` feature (pulls DataFusion) — rebuild: cargo build \
         -p tessera-cli --features sql"
            .into(),
    ))
}

/// Build a 1-product [`ingest_spec::IngestSpec`] from a per-format CLI subcommand + run it through
/// the engine. The engine writes the sealed `.tsra` to `<spec_out>/<id>.tsra`; we then rename it
/// to the user-supplied `out` path (single-product subcommands speak in file paths, not directories).
/// This is the "net code delete" the per-format paths now share with the spec-driven path —
/// dispatch logic lives in one place (`engine::dispatch`), not duplicated here.
fn run_ingest(src: IngestSrc) -> tessera_core::Result<()> {
    let (spec, user_out) = ingest_src_to_spec(src)?;
    let staging = tempfile::tempdir().map_err(|e| {
        tessera_core::Error::Invalid(format!("tessera ingest: create staging dir: {e}"))
    })?;
    let cfg = WriteConfig::for_system();
    let coll = engine::run(
        &spec,
        std::path::Path::new("cli-inline-spec"),
        staging.path(),
        &cfg,
        engine::DEFAULT_STREAM_THRESHOLD_BYTES,
    )?;
    // Move the single produced `.tsra` to the user's `out` path. The engine names files by
    // sanitized id (`blake3_<hex>.tsra`); we read that name back out of the sealed collection.
    let member = coll.members.first().ok_or_else(|| {
        tessera_core::Error::Invalid("tessera ingest: engine produced no member".into())
    })?;
    let sanitized = member.reference.replace([':', '/', '\\'], "_");
    let from = staging.path().join(format!("{sanitized}.tsra"));
    if let Some(parent) = user_out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                tessera_core::Error::Invalid(format!(
                    "tessera ingest: create {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    // Try fs::rename first (atomic on the same FS); fall back to copy+remove across mountpoints
    // (the staging tempdir may live on a different filesystem than `out`).
    if std::fs::rename(&from, &user_out).is_err() {
        std::fs::copy(&from, &user_out).map_err(|e| {
            tessera_core::Error::Invalid(format!(
                "tessera ingest: copy {} -> {}: {e}",
                from.display(),
                user_out.display()
            ))
        })?;
    }
    println!(
        "ingested {} ({}) -> {}",
        member.reference,
        spec.products[0].schema,
        user_out.display()
    );
    Ok(())
}

/// Translate a per-format CLI subcommand into a 1-product spec + the caller's chosen output path.
fn ingest_src_to_spec(src: IngestSrc) -> tessera_core::Result<(ingest_spec::IngestSpec, PathBuf)> {
    use ingest_spec::{
        CollectionMeta, FormatOptions, IngestSpec, ProductSpec, SpecMeta, StreamingMode,
        DEFAULT_BLOCK_PREFIX, DEFAULT_ROW_INDEX, DEFAULT_SLAB_ROWS,
    };
    use tessera_core::collection::Role;
    Ok(match src {
        IngestSrc::Dicom {
            input,
            out,
            name,
            timestamp,
            deidentify,
            recipient,
            source_label,
            meta,
        } => (
            IngestSpec {
                collection: CollectionMeta {
                    name: name.clone(),
                    description: None,
                    timestamp: timestamp.clone(),
                    study: None,
                },
                spec: SpecMeta::default(),
                products: vec![ProductSpec {
                    name,
                    role: Role::Raw,
                    schema: "recon".into(),
                    description: None,
                    derived_from: Vec::new(),
                    source_label,
                    metadata: parse_meta(&meta)?,
                    options: FormatOptions::Dicom {
                        input,
                        deidentify,
                        recipients: recipient,
                    },
                }],
            },
            out,
        ),
        IngestSrc::DicomSeries {
            inputs,
            out,
            name,
            timestamp,
            deidentify,
            recipient,
            source_label,
            meta,
        } => (
            IngestSpec {
                collection: CollectionMeta {
                    name: name.clone(),
                    description: None,
                    timestamp: timestamp.clone(),
                    study: None,
                },
                spec: SpecMeta::default(),
                products: vec![ProductSpec {
                    name,
                    role: Role::Raw,
                    schema: "recon".into(),
                    description: None,
                    derived_from: Vec::new(),
                    source_label,
                    metadata: parse_meta(&meta)?,
                    options: FormatOptions::DicomSeries {
                        inputs,
                        deidentify,
                        recipients: recipient,
                    },
                }],
            },
            out,
        ),
        IngestSrc::GeHdf5 {
            input,
            out,
            name,
            timestamp,
            dataset,
            source_label,
            meta,
        } => (
            IngestSpec {
                collection: CollectionMeta {
                    name: name.clone(),
                    description: None,
                    timestamp: timestamp.clone(),
                    study: None,
                },
                spec: SpecMeta::default(),
                products: vec![ProductSpec {
                    name,
                    role: Role::Raw,
                    schema: "listmode".into(),
                    description: None,
                    derived_from: Vec::new(),
                    source_label,
                    metadata: parse_meta(&meta)?,
                    options: FormatOptions::HdfCompound {
                        input,
                        dataset,
                        row_index: DEFAULT_ROW_INDEX.into(),
                        block_prefix: DEFAULT_BLOCK_PREFIX.into(),
                        streaming: StreamingMode::Auto,
                        slab_rows: DEFAULT_SLAB_ROWS,
                    },
                }],
            },
            out,
        ),
        IngestSrc::Blob {
            input,
            out,
            name,
            timestamp,
            media_type,
            source_label,
            meta,
        } => (
            IngestSpec {
                collection: CollectionMeta {
                    name: name.clone(),
                    description: None,
                    timestamp: timestamp.clone(),
                    study: None,
                },
                spec: SpecMeta::default(),
                products: vec![ProductSpec {
                    name,
                    role: Role::Raw,
                    schema: "blob".into(),
                    description: None,
                    derived_from: Vec::new(),
                    source_label,
                    metadata: parse_meta(&meta)?,
                    options: FormatOptions::Blob { input, media_type },
                }],
            },
            out,
        ),
    })
}

/// Options bag for the `tessera ingest --spec` runner.
struct IngestSpecOpts {
    spec_path: PathBuf,
    out_dir: Option<PathBuf>,
    workers: Option<usize>,
    ram_budget: Option<String>,
    auto: bool,
    stream_threshold: Option<String>,
}

/// Run `tessera ingest --spec FILE` — parse the spec, build the WriteConfig from runtime knobs
/// (`--workers`, `--ram-budget`, `--auto`; NEVER from the spec, per ADR-0035 hole #4), then
/// dispatch to [`engine::run`]. Prints a one-line summary per member + the sealed collection id.
fn run_ingest_spec(opts: IngestSpecOpts) -> tessera_core::Result<()> {
    let parsed = ingest_spec::parse(&opts.spec_path)?;
    let out_dir = opts.out_dir.unwrap_or_else(|| PathBuf::from("ingest-out"));
    let mut cfg = WriteConfig::for_system();
    if let Some(n) = opts.workers {
        cfg = cfg.workers(n);
    }
    if let Some(s) = opts.ram_budget {
        cfg = cfg.ram_budget(parse_byte_size(&s)?);
    }
    if opts.auto {
        // The honest knee model needs measured read/encode rates; without a fixture here we just
        // surface the request and stick to defaults (the bench subcommand is where the live
        // measurement lives). Promoting `--auto` to a no-op keeps the CLI shape stable without
        // pretending we measured.
        eprintln!(
            "note: --auto on `tessera ingest --spec` keeps the for_system() defaults (the \
             measured knee model lives on `tessera bench write --auto`; the spec engine has no \
             fixture to time against)"
        );
    }
    let threshold = match opts.stream_threshold {
        Some(s) => parse_byte_size(&s)?,
        None => engine::DEFAULT_STREAM_THRESHOLD_BYTES,
    };
    let coll = engine::run(&parsed, &opts.spec_path, &out_dir, &cfg, threshold)?;
    println!(
        "ingested collection {} ({} members) -> {}",
        coll.id,
        coll.members.len(),
        out_dir.display()
    );
    for m in &coll.members {
        let sanitized = m.reference.replace([':', '/', '\\'], "_");
        println!("  - {} -> {sanitized}.tsra", m.reference);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::array::{ArrayBlock, ArraySpec};
    use tessera_core::ProductBuilder;
    use tessera_io::{pack, BlockPayload};

    /// Drift guard for the grouped `--help` (#249): every real subcommand must appear in the
    /// hand-authored `HELP_TEMPLATE`, so adding a `Cmd` variant without listing it fails CI.
    #[test]
    fn every_subcommand_is_grouped_in_help() {
        use clap::CommandFactory;
        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            if name == "help" {
                continue; // clap's built-in, intentionally not in the grouped block
            }
            // Line-anchored (indent + name) so short names like `ls` can't false-match a substring.
            assert!(
                HELP_TEMPLATE.contains(&format!("\n  {name} ")),
                "subcommand '{name}' is missing from the grouped HELP_TEMPLATE"
            );
        }
    }

    fn sample_tsra(path: &std::path::Path) {
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![16, 16, 16], "int16"));
        let payload = serde_json::to_vec(&vol.spec).unwrap();
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field("modality", serde_json::json!("CT"));
        let sealed = b.seal().unwrap();
        pack(&sealed, &[BlockPayload::new("volume", payload)], path).unwrap();
    }

    #[test]
    fn verify_inspect_unpack_pack_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample_tsra(&tsra);

        run(Cmd::Verify { file: tsra.clone() }).unwrap();
        run(Cmd::Inspect {
            file: tsra.clone(),
            full: false,
        })
        .unwrap();
        run(Cmd::Schema {
            file: tsra.clone(),
            json: false,
        })
        .unwrap();

        let exploded = dir.path().join("exploded");
        run(Cmd::Unpack {
            file: tsra.clone(),
            outdir: exploded.clone(),
        })
        .unwrap();
        assert!(exploded.join("manifest.json").exists());
        assert!(exploded.join("blocks/volume").exists());

        let repacked = dir.path().join("repacked.tsra");
        run(Cmd::Pack {
            dir: exploded,
            out: repacked.clone(),
        })
        .unwrap();
        run(Cmd::Verify { file: repacked }).unwrap();
    }

    #[test]
    fn verify_rejects_a_corrupt_container() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.tsra");
        std::fs::write(&bad, b"not a zip at all").unwrap();
        assert!(run(Cmd::Verify { file: bad }).is_err());
    }

    // Mirrors the GE 3-photon compound record (HDF5 maps by member name on read).
    #[repr(C)]
    #[derive(hdf5_metno::H5Type, Clone, Copy)]
    struct Rec3p {
        ms: u32,
        id: [u16; 3],
        en: [f32; 3],
        vtx: [f32; 3],
        lt: f32,
    }

    #[test]
    fn ingest_ge_hdf5_then_verify_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST.h5");
        let recs: Vec<Rec3p> = (0..50)
            .map(|k| Rec3p {
                ms: k,
                id: [k as u16, 1, 2],
                en: [511.0, 511.0, 511.0],
                vtx: [0.0, 0.0, 0.0],
                lt: 1.0,
            })
            .collect();
        let f = hdf5_metno::File::create(&h5).unwrap();
        f.new_dataset::<Rec3p>()
            .shape(recs.len())
            .create("events_3p")
            .unwrap()
            .write(&recs)
            .unwrap();
        drop(f);

        let out = dir.path().join("lm.tsra");
        run(Cmd::Ingest {
            spec: None,
            out: None,
            workers: None,
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: Some(IngestSrc::GeHdf5 {
                input: h5,
                out: out.clone(),
                name: "DP06-lm".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                dataset: "events_3p".into(),
                source_label: None,
                meta: vec![],
            }),
        })
        .unwrap();

        // the ingested product verifies (seal + block digest) and is schema-valid. The engine adds
        // an `ingested_via_spec` edge on every member it produces (including the synthesised
        // 1-product spec the per-format CLI builds), so the source count goes from 1 to 2.
        run(Cmd::Verify { file: out.clone() }).unwrap();
        run(Cmd::Inspect {
            file: out.clone(),
            full: false,
        })
        .unwrap();
        let r = Reader::open(&out).unwrap();
        assert_eq!(r.manifest().product, "listmode");
        assert_eq!(r.manifest().sources.len(), 2);
        assert!(r
            .manifest()
            .sources
            .iter()
            .any(|s| s.role == engine::SPEC_PROVENANCE_ROLE));
    }

    #[test]
    fn ingest_spec_seals_a_collection_and_each_member_verifies() {
        // End-to-end of `tessera ingest --spec FILE`: write a 2-product TOML over two synthetic
        // .h5 fixtures with a derived_from edge; run the CLI; assert the collection.json + both
        // member .tsra files exist + verify; assert the derived member carries the expected
        // `derived_from` + `ingested_via_spec` provenance edges.
        let dir = tempfile::tempdir().unwrap();
        let h5_a = dir.path().join("a.h5");
        let h5_b = dir.path().join("b.h5");
        for h5 in [&h5_a, &h5_b] {
            let recs: Vec<Rec3p> = (0..50)
                .map(|k| Rec3p {
                    ms: k as u32,
                    id: [k as u16, 1, 2],
                    en: [511.0, 511.0, 511.0],
                    vtx: [0.0, 0.0, 0.0],
                    lt: 1.0,
                })
                .collect();
            let f = hdf5_metno::File::create(h5).unwrap();
            f.new_dataset::<Rec3p>()
                .shape(recs.len())
                .create("events_3p")
                .unwrap()
                .write(&recs)
                .unwrap();
        }
        let spec_path = dir.path().join("spec.toml");
        let toml_text = format!(
            r#"
[collection]
name = "DP06-study"
description = "synth spec"
timestamp = "2024-01-01T00:00:00Z"
study = "DP06-2024-01"

[[product]]
name = "raw-3p"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{a}"
dataset = "events_3p"
streaming = "batch"

[[product]]
name = "derived-3p"
role = "derived"
schema = "listmode"
derived_from = ["raw-3p"]
format = "hdf-compound"
input = "{b}"
dataset = "events_3p"
streaming = "batch"
"#,
            a = h5_a.display(),
            b = h5_b.display()
        );
        std::fs::write(&spec_path, toml_text).unwrap();

        let out_dir = dir.path().join("out");
        run(Cmd::Ingest {
            spec: Some(spec_path.clone()),
            out: Some(out_dir.clone()),
            workers: Some(2),
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: None,
        })
        .unwrap();

        // 1. collection.json exists + verifies.
        let coll_json = std::fs::read_to_string(out_dir.join("collection.json")).unwrap();
        let coll = tessera_core::Collection::from_json_verified(&coll_json).unwrap();
        assert_eq!(coll.members.len(), 2);
        assert_eq!(coll.study.as_deref(), Some("DP06-2024-01"));

        // 2. every member's .tsra exists + verifies + carries the expected spec edge.
        for m in &coll.members {
            let p = out_dir.join(format!("{}.tsra", m.reference.replace([':', '/'], "_")));
            assert!(p.exists(), "missing {}", p.display());
            run(Cmd::Verify { file: p.clone() }).unwrap();
            let r = Reader::open(&p).unwrap();
            assert!(r
                .manifest()
                .sources
                .iter()
                .any(|s| s.role == engine::SPEC_PROVENANCE_ROLE));
        }

        // 3. the derived member's `derived_from` edge pins the raw's manifest_hash → chain verifies.
        let raw_id = &coll.members[0].reference;
        let derived_id = &coll.members[1].reference;
        let raw_path = out_dir.join(format!("{}.tsra", raw_id.replace([':', '/'], "_")));
        let derived_path = out_dir.join(format!("{}.tsra", derived_id.replace([':', '/'], "_")));
        let raw_m = Reader::open(&raw_path).unwrap().manifest().clone();
        let derived_m = Reader::open(&derived_path).unwrap().manifest().clone();
        let mut resolver: std::collections::BTreeMap<String, tessera_core::Manifest> =
            std::collections::BTreeMap::new();
        resolver.insert(raw_m.id.clone(), raw_m);
        tessera_core::provenance::verify_chain(&derived_m, &resolver).unwrap();

        // 4. determinism: a second run into a fresh dir produces the same collection identity +
        //    seal (proves --workers / --ram-budget are runtime knobs that never change bytes).
        let out2 = dir.path().join("out2");
        run(Cmd::Ingest {
            spec: Some(spec_path),
            out: Some(out2.clone()),
            workers: Some(4), // different worker count → SAME bytes
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: None,
        })
        .unwrap();
        let coll2_json = std::fs::read_to_string(out2.join("collection.json")).unwrap();
        let coll2 = tessera_core::Collection::from_json_verified(&coll2_json).unwrap();
        assert_eq!(coll.id, coll2.id);
        assert_eq!(coll.content_hash, coll2.content_hash);
        assert_eq!(coll.manifest_hash, coll2.manifest_hash);
    }

    #[test]
    fn ingest_rejects_spec_and_subcommand_together() {
        // The clap conflict-with-subcommand limitation is enforced at runtime — this guards that.
        let dir = tempfile::tempdir().unwrap();
        let spec_path = dir.path().join("spec.toml");
        std::fs::write(
            &spec_path,
            "[collection]\nname = \"x\"\ntimestamp = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let err = run(Cmd::Ingest {
            spec: Some(spec_path),
            out: None,
            workers: None,
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: Some(IngestSrc::Dicom {
                input: dir.path().join("nope.dcm"),
                out: dir.path().join("nope.tsra"),
                name: "x".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                deidentify: false,
                recipient: vec![],
                source_label: None,
                meta: vec![],
            }),
        })
        .unwrap_err();
        assert!(
            format!("{err}").contains("mutually exclusive"),
            "expected mutual-exclusion error, got: {err}"
        );
    }

    #[test]
    fn ingest_dicom_series_with_deidentify_reaches_the_de_id_read_path() {
        // The CLI `dicom-series --deidentify` variant must reach the engine's de-id path (no longer
        // a hard reject). We give it bogus input paths so the call fails INSIDE the de-id read path
        // (`read_series_deidentified` → `open_file`) — proving the wiring without DICOM fixtures.
        // The non-deid path would fail the same way at `read_series`, but with a different error: we
        // assert the failure is the DICOM open error (i.e. we reached the reader), NOT the previous
        // hard-reject "not yet supported" string.
        let dir = tempfile::tempdir().unwrap();
        let err = run(Cmd::Ingest {
            spec: None,
            out: None,
            workers: None,
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: Some(IngestSrc::DicomSeries {
                inputs: vec![dir.path().join("a.dcm"), dir.path().join("b.dcm")],
                out: dir.path().join("series.tsra"),
                name: "DP06-ct".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                deidentify: true,
                recipient: vec![],
                source_label: None,
                meta: vec![],
            }),
        })
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            !msg.contains("not yet supported"),
            "series-deid must no longer be rejected up-front, got: {msg}"
        );
        assert!(
            msg.contains("dicom"),
            "expected the failure to come from inside the DICOM read path, got: {msg}"
        );
    }

    /// End-to-end: `dicom-series --deidentify` over real (synthetic) DICOM slices must SEAL a recon
    /// product whose `ingested_from` reference is the `--source-label` (not the PHI-bearing paths).
    /// Proves both load-bearing changes — wired de-id AND `--source-label` — work together on the
    /// production path.
    #[test]
    fn ingest_dicom_series_deidentify_with_source_label_seals_product() {
        use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
        use dicom::object::meta::FileMetaTableBuilder;
        use dicom::object::InMemDicomObject;

        let dir = tempfile::tempdir().unwrap();
        // Two minimal CT slices, each carrying PHI.
        let write = |path: &std::path::Path, instance: i32, base: u16, patient: &str| {
            let pixels: Vec<u16> = (0..64).map(|k| base + k as u16).collect();
            let obj = InMemDicomObject::from_element_iter([
                DataElement::new(Tag(0x0008, 0x0060), VR::CS, PrimitiveValue::from("CT")),
                DataElement::new(Tag(0x0028, 0x0010), VR::US, PrimitiveValue::from(8u16)),
                DataElement::new(Tag(0x0028, 0x0011), VR::US, PrimitiveValue::from(8u16)),
                DataElement::new(
                    Tag(0x0020, 0x0013),
                    VR::IS,
                    PrimitiveValue::from(instance.to_string()),
                ),
                DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
                DataElement::new(
                    Tag(0x0028, 0x0004),
                    VR::CS,
                    PrimitiveValue::from("MONOCHROME2"),
                ),
                DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
                DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
                DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
                DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
                DataElement::new(Tag(0x0028, 0x1052), VR::DS, PrimitiveValue::from("-1024")),
                DataElement::new(Tag(0x0028, 0x1053), VR::DS, PrimitiveValue::from("1")),
                DataElement::new(Tag(0x0010, 0x0010), VR::PN, PrimitiveValue::from(patient)),
                DataElement::new(
                    Tag(0x0010, 0x0020),
                    VR::LO,
                    PrimitiveValue::from(format!("PID-{instance}")),
                ),
                DataElement::new(
                    Tag(0x7FE0, 0x0010),
                    VR::OW,
                    PrimitiveValue::U16(pixels.into()),
                ),
            ]);
            let meta = FileMetaTableBuilder::new()
                .transfer_syntax("1.2.840.10008.1.2.1")
                .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
                .media_storage_sop_instance_uid(format!("1.2.3.deid.{instance}"))
                .implementation_class_uid("1.2.826.0.1.3680043.tessera")
                .build()
                .unwrap();
            obj.with_exact_meta(meta).write_to_file(path).unwrap();
        };
        let p1 = dir.path().join("phi1.dcm");
        let p2 = dir.path().join("phi2.dcm");
        write(&p1, 1, 1000, "DOE^JOHN");
        write(&p2, 2, 2000, "DOE^JANE");

        let out = dir.path().join("series.tsra");
        run(Cmd::Ingest {
            spec: None,
            out: None,
            workers: None,
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: Some(IngestSrc::DicomSeries {
                inputs: vec![p1.clone(), p2.clone()],
                out: out.clone(),
                name: "DP06-ct".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                deidentify: true,
                recipient: vec![],
                source_label: Some("DUPLET-07/CT".into()),
                meta: vec![],
            }),
        })
        .unwrap();

        // The sealed product opens + verifies + carries the clean source label (NOT a slice path).
        run(Cmd::Verify { file: out.clone() }).unwrap();
        let m = Reader::open(&out).unwrap().manifest().clone();
        let ingested_from = m
            .sources
            .iter()
            .find(|s| s.role == "ingested_from")
            .expect("ingested_from edge on sealed manifest");
        assert_eq!(ingested_from.reference, "DUPLET-07/CT");
        assert!(
            !ingested_from
                .reference
                .contains(p1.file_name().unwrap().to_str().unwrap()),
            "PHI-bearing slice path must not appear in the sealed manifest, got: {}",
            ingested_from.reference
        );
    }

    /// Write one minimal PHI-bearing CT `.dcm` at `path` (shared by the crypto-shred tests). The
    /// patient name / id are the sentinels the tests assert never leak into the sealed product.
    #[cfg(test)]
    fn write_phi_dcm(path: &std::path::Path, patient: &str, pid: &str) {
        use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
        use dicom::object::meta::FileMetaTableBuilder;
        use dicom::object::InMemDicomObject;
        let pixels: Vec<u16> = (0..64).map(|k| 1000 + k as u16).collect();
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0008, 0x0060), VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(Tag(0x0028, 0x0010), VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(Tag(0x0028, 0x0011), VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(Tag(0x0020, 0x0013), VR::IS, PrimitiveValue::from("1")),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(Tag(0x0028, 0x1052), VR::DS, PrimitiveValue::from("-1024")),
            DataElement::new(Tag(0x0028, 0x1053), VR::DS, PrimitiveValue::from("1")),
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, PrimitiveValue::from(patient)),
            DataElement::new(Tag(0x0010, 0x0020), VR::LO, PrimitiveValue::from(pid)),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid("1.2.3.cs.1")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        obj.with_exact_meta(meta).write_to_file(path).unwrap();
    }

    /// Naive substring search over bytes (test-only; the `twoway` crate isn't a dep).
    fn twoway_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// `path` with an added extension component (`recipient.key` → `recipient.key.pub`).
    fn with_ext(path: &std::path::Path, ext: &str) -> std::path::PathBuf {
        let mut s = path.as_os_str().to_os_string();
        s.push(".");
        s.push(ext);
        std::path::PathBuf::from(s)
    }

    /// ADR-0047 crypto-shred de-identification, end to end: the sealed product is PHI-free, yet a
    /// recipient private key recovers the full identity — and a wrong key, or a `shred`, cannot.
    #[test]
    fn crypto_shred_deidentifies_yet_a_key_holder_can_reidentify() {
        const PHI_NAME: &str = "SHREDTEST^PATIENT";
        const PHI_ID: &str = "MRN-SHRED-42";
        let dir = tempfile::tempdir().unwrap();
        let dcm = dir.path().join("phi.dcm");
        write_phi_dcm(&dcm, PHI_NAME, PHI_ID);

        // A recipient age keypair (what `tessera keygen --age` writes).
        let key = dir.path().join("recipient.key");
        run(Cmd::Keygen {
            out: key.clone(),
            age: true,
        })
        .unwrap();
        let recipient_pub = std::fs::read_to_string(with_ext(&key, "pub"))
            .unwrap()
            .trim()
            .to_string();

        // Crypto-shred ingest.
        let shredded = dir.path().join("cs.tsra");
        run(Cmd::Ingest {
            spec: None,
            out: None,
            workers: None,
            ram_budget: None,
            auto: false,
            stream_threshold: None,
            src: Some(IngestSrc::Dicom {
                input: dcm.clone(),
                out: shredded.clone(),
                name: "cs".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                deidentify: false,
                recipient: vec![recipient_pub],
                source_label: Some("STUDY-1/CT".into()),
                meta: vec![],
            }),
        })
        .unwrap();

        // The sealed product verifies and is PHI-free: the whole container bytes carry no plaintext
        // patient name / id (they live only in the encrypted envelope).
        run(Cmd::Verify {
            file: shredded.clone(),
        })
        .unwrap();
        let raw = std::fs::read(&shredded).unwrap();
        assert!(
            !twoway_contains(&raw, PHI_NAME.as_bytes()),
            "patient name leaked into the sealed container"
        );
        assert!(
            !twoway_contains(&raw, PHI_ID.as_bytes()),
            "patient id leaked into the sealed container"
        );
        // The identity envelope rides in aux/ (outside the seal).
        let r = Reader::open(&shredded).unwrap();
        assert!(
            r.aux_names()
                .iter()
                .any(|n| n == tessera_ingest::identity::AUX_IDENTITY_NAME),
            "aux identity envelope missing"
        );
        drop(r);

        // A key holder re-identifies: the recovered header carries the original PHI.
        let recovered = dir.path().join("recovered.json");
        run(Cmd::Reidentify {
            file: shredded.clone(),
            identity: key.clone(),
            out: Some(recovered.clone()),
        })
        .unwrap();
        let recovered_json = std::fs::read_to_string(&recovered).unwrap();
        assert!(
            recovered_json.contains(PHI_NAME),
            "re-identify lost the name"
        );
        assert!(recovered_json.contains(PHI_ID), "re-identify lost the id");

        // A different key cannot.
        let wrong = dir.path().join("wrong.key");
        run(Cmd::Keygen {
            out: wrong.clone(),
            age: true,
        })
        .unwrap();
        assert!(run(Cmd::Reidentify {
            file: shredded.clone(),
            identity: wrong,
            out: None,
        })
        .is_err());

        // Shred: the envelope is gone, the seal still verifies, and even the right key can't recover.
        run(Cmd::Shred {
            file: shredded.clone(),
        })
        .unwrap();
        run(Cmd::Verify {
            file: shredded.clone(),
        })
        .unwrap();
        let r = Reader::open(&shredded).unwrap();
        assert!(
            !r.aux_names()
                .iter()
                .any(|n| n == tessera_ingest::identity::AUX_IDENTITY_NAME),
            "identity envelope survived shred"
        );
        drop(r);
        assert!(run(Cmd::Reidentify {
            file: shredded,
            identity: key,
            out: None,
        })
        .is_err());
    }

    /// ADR-0047 §3: a crypto-shred product's DATA identity (`id` + `content_hash`) is identical to a
    /// plain `--deidentify` product of the same input — the de-identified pixels + metadata are the
    /// same, and the encrypted envelope rides outside the seal. The `manifest_hash` may differ by the
    /// `ingested_via_spec` provenance edge alone: the recipient directive is recorded (auditable) in
    /// provenance, which is correct — the *data* is identical, the *provenance* honestly is not.
    #[test]
    fn crypto_shred_seal_equals_plain_deidentify() {
        let dir = tempfile::tempdir().unwrap();
        let dcm = dir.path().join("phi.dcm");
        write_phi_dcm(&dcm, "EQ^PATIENT", "MRN-EQ-1");
        let key = dir.path().join("r.key");
        run(Cmd::Keygen {
            out: key.clone(),
            age: true,
        })
        .unwrap();
        let pubkey = std::fs::read_to_string(with_ext(&key, "pub"))
            .unwrap()
            .trim()
            .to_string();

        let ingest = |out: &std::path::Path, deidentify: bool, recipient: Vec<String>| {
            run(Cmd::Ingest {
                spec: None,
                out: None,
                workers: None,
                ram_budget: None,
                auto: false,
                stream_threshold: None,
                src: Some(IngestSrc::Dicom {
                    input: dcm.clone(),
                    out: out.to_path_buf(),
                    name: "eq".into(),
                    timestamp: "2024-01-01T00:00:00Z".into(),
                    deidentify,
                    recipient,
                    source_label: Some("S/CT".into()),
                    meta: vec![],
                }),
            })
            .unwrap();
        };
        let cs = dir.path().join("cs.tsra");
        let deid = dir.path().join("deid.tsra");
        ingest(&cs, false, vec![pubkey]);
        ingest(&deid, true, vec![]);

        let cs_m = Reader::open(&cs).unwrap().manifest().clone();
        let deid_m = Reader::open(&deid).unwrap().manifest().clone();
        // The de-identified DATA is identical (same lineage id + same block-merkle content_hash).
        assert_eq!(cs_m.id, deid_m.id);
        assert_eq!(cs_m.content_hash, deid_m.content_hash);
        // The only seal-covered difference is the `ingested_via_spec` provenance edge (the recipient
        // directive is recorded there) — every OTHER source edge is byte-identical.
        let strip_spec = |m: &tessera_core::Manifest| {
            m.sources
                .iter()
                .filter(|s| s.role != "ingested_via_spec")
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(strip_spec(&cs_m), strip_spec(&deid_m));
        assert_eq!(cs_m.metadata, deid_m.metadata);
        assert_eq!(cs_m.extra, deid_m.extra);
    }

    #[test]
    fn export_ro_crate_and_datacite_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample_tsra(&tsra);
        run(Cmd::Export {
            file: tsra.clone(),
            format: "ro-crate".into(),
        })
        .unwrap();
        run(Cmd::Export {
            file: tsra,
            format: "datacite".into(),
        })
        .unwrap();
    }
}
