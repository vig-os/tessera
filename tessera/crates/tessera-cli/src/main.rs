//! `tessera` — pack / unpack / verify / inspect `.tsra` products (ROADMAP P4, #205).
//!
//! A thin shell over `tessera-core` (format/spine) + `tessera-io` (container). Every command
//! that opens a `.tsra` verifies its magic + manifest seal; `verify` additionally checks every
//! block's stored bytes against its recorded digest.

mod bench;

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

/// CLI-only erased view over a `Reader<R>` — narrows the trait-object surface to the two
/// methods `inspect` / `verify` actually need (manifest access + per-block read). Hides the
/// concrete `R` (local `File` vs `ObjectStoreReader`) behind a single boxed handle.
trait TsraSource {
    fn manifest(&self) -> &tessera_core::Manifest;
    fn read_block_by_name(&mut self, name: &str) -> tessera_core::Result<Vec<u8>>;
    fn block_names(&self) -> Vec<String>;
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
}

#[derive(Parser)]
#[command(name = "tessera", version, about = "Tessera FAIR data-product CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a human summary of a `.tsra`'s manifest (id, product, blocks, hashes).
    ///
    /// With the `cloud` feature enabled, `file` also accepts `s3://<bucket>/<key>` or
    /// `http(s)://<host>/<key>` — the manifest is read via range-GET over the wire.
    Inspect { file: PathBuf },
    /// Open + fully verify a `.tsra` (magic, manifest seal, every block digest). Exit 0 if valid.
    ///
    /// With the `cloud` feature, `file` also accepts `s3://` / `http(s)://` URLs.
    Verify { file: PathBuf },
    /// Explode a `.tsra` into a directory (`manifest.json` + `blocks/<name>`).
    Unpack { file: PathBuf, outdir: PathBuf },
    /// Pack an exploded directory (`manifest.json` + `blocks/`) into a sealed `.tsra`.
    Pack { dir: PathBuf, out: PathBuf },
    /// Validate a `.tsra`'s manifest against its declared product schema (required fields/blocks).
    Schema { file: PathBuf },
    /// Ingest a vendor acquisition file into a sealed `.tsra` product (normalise at the door), or
    /// run a declarative ingest spec (`--spec FILE`) into a sealed collection of `.tsra` products.
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
        #[command(subcommand)]
        fmt: ExportFmt,
    },
    /// Sign a sealed `.tsra` → writes a `<file>.sig.json` sidecar (ed25519 over `manifest_hash`).
    Sign {
        /// The sealed `.tsra` to sign.
        file: PathBuf,
        /// Hex-encoded 32-byte ed25519 signing-key (seed) file.
        #[arg(long)]
        key: PathBuf,
        /// Optional signer identity recorded in the signature (e.g. an ORCID iD URL).
        #[arg(long)]
        signer: Option<String>,
    },
    /// Verify a sealed `.tsra` against its `<file>.sig.json` sidecar + a trusted public key. Exit 0 if valid.
    VerifySig {
        /// The sealed `.tsra` to verify.
        file: PathBuf,
        /// Hex-encoded 32-byte ed25519 public-key file (obtained out-of-band from the signer).
        #[arg(long)]
        pubkey: PathBuf,
    },
    /// Bench the write engine on this host — drives the real `StreamWriter`/`TableStreamWriter`,
    /// reports throughput + peak RSS so an operator can size RAM/threads for their acquisition rate.
    Bench {
        #[command(subcommand)]
        action: BenchAction,
    },
}

#[derive(Subcommand)]
enum BenchAction {
    /// Bench the streaming write engine (events/s + MB/s + peak RAM) — synthetic or real `.h5`.
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
enum ExportFmt {
    /// RO-Crate metadata descriptor (`ro-crate-metadata.json` shape).
    RoCrate { file: PathBuf },
    /// DataCite metadata record (for DOI minting / InvenioRDM).
    Datacite { file: PathBuf },
}

#[derive(Subcommand)]
enum IngestSrc {
    /// DICOM image/series → `recon` product (lossless int16 + rescale/units/modality + provenance).
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
    },
    /// GE listmode HDF5 → `listmode` product (compound events → columnar; the #193 transpose).
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
    },
}

fn main() -> ExitCode {
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
        Cmd::Inspect { file } => {
            let r = open_local_or_url(&file)?;
            let m = r.manifest();
            println!("tessera {} · product={}", m.tessera_version, m.product);
            println!("id            {}", m.id);
            println!("name          {}", m.name);
            println!("timestamp     {}", m.timestamp);
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
                    println!("  - {} <- {}", s.role, s.reference);
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
        Cmd::Schema { file } => {
            let r = Reader::open(&file)?;
            let m = r.manifest();
            let reg = SchemaRegistry::builtin();
            match reg.get(&m.product) {
                Some(s) => println!(
                    "product '{}' — built-in schema v{} ({})",
                    m.product, s.version, s.description
                ),
                None => println!(
                    "product '{}' — unknown schema (open-world: validation is permissive)",
                    m.product
                ),
            }
            reg.validate(m)?; // typed Invalid error naming the first missing required field/block
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
        Cmd::Export { fmt } => {
            let record = match fmt {
                ExportFmt::RoCrate { file } => {
                    tessera_core::export::ro_crate(Reader::open(&file)?.manifest())
                }
                ExportFmt::Datacite { file } => {
                    tessera_core::export::datacite(Reader::open(&file)?.manifest())
                }
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&record).map_err(tessera_core::Error::from)?
            );
            Ok(())
        }
        Cmd::Sign { file, key, signer } => {
            let key_hex = std::fs::read_to_string(&key).map_err(tessera_core::Error::from)?;
            let sk = tessera_core::signing::signing_key_from_hex(&key_hex)?;
            let env = tessera_io::sign_tsra(&file, &sk, signer)?;
            println!("OK  signed {}", file.display());
            println!(
                "  sidecar   {}",
                tessera_io::sign::sidecar_path(&file).display()
            );
            println!("  key_id    {}", env.key_id);
            if let Some(s) = &env.signer {
                println!("  signer    {s}");
            }
            Ok(())
        }
        Cmd::VerifySig { file, pubkey } => {
            let pub_hex = std::fs::read_to_string(&pubkey).map_err(tessera_core::Error::from)?;
            let vk = tessera_core::signing::verifying_key_from_hex(&pub_hex)?;
            // 1. the signature attests the manifest (and thus every recorded block digest + metadata).
            if !tessera_io::verify_tsra(&file, &vk)? {
                return Err(tessera_core::Error::Invalid(format!(
                    "signature INVALID for {}",
                    file.display()
                )));
            }
            // 2. also re-read every block's payload vs its recorded digest, so a payload swap that left
            //    the manifest untouched is still caught — verify-sig answers "authentic AND intact".
            let mut r = Reader::open(&file)?;
            let n = r.manifest().blocks.len();
            for name in r.block_names() {
                r.read_block(&name)?;
            }
            println!("OK  {} signature valid + {n} blocks intact", file.display());
            Ok(())
        }
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
                    metadata: std::collections::BTreeMap::new(),
                    options: FormatOptions::Dicom { input, deidentify },
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
                    metadata: std::collections::BTreeMap::new(),
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
        run(Cmd::Inspect { file: tsra.clone() }).unwrap();
        run(Cmd::Schema { file: tsra.clone() }).unwrap();

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
            }),
        })
        .unwrap();

        // the ingested product verifies (seal + block digest) and is schema-valid. The engine adds
        // an `ingested_via_spec` edge on every member it produces (including the synthesised
        // 1-product spec the per-format CLI builds), so the source count goes from 1 to 2.
        run(Cmd::Verify { file: out.clone() }).unwrap();
        run(Cmd::Inspect { file: out.clone() }).unwrap();
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
            }),
        })
        .unwrap_err();
        assert!(
            format!("{err}").contains("mutually exclusive"),
            "expected mutual-exclusion error, got: {err}"
        );
    }

    #[test]
    fn export_ro_crate_and_datacite_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample_tsra(&tsra);
        run(Cmd::Export {
            fmt: ExportFmt::RoCrate { file: tsra.clone() },
        })
        .unwrap();
        run(Cmd::Export {
            fmt: ExportFmt::Datacite { file: tsra },
        })
        .unwrap();
    }
}
