//! `tessera` — pack / unpack / verify / inspect `.tsra` products (ROADMAP P4, #205).
//!
//! A thin shell over `tessera-core` (format/spine) + `tessera-io` (container). Every command
//! that opens a `.tsra` verifies its magic + manifest seal; `verify` additionally checks every
//! block's stored bytes against its recorded digest.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tessera_core::SchemaRegistry;
use tessera_ingest::{dicom, ge_hdf5};
use tessera_io::{pack, pack_dir, unpack, Reader};

#[derive(Parser)]
#[command(name = "tessera", version, about = "Tessera FAIR data-product CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a human summary of a `.tsra`'s manifest (id, product, blocks, hashes).
    Inspect { file: PathBuf },
    /// Open + fully verify a `.tsra` (magic, manifest seal, every block digest). Exit 0 if valid.
    Verify { file: PathBuf },
    /// Explode a `.tsra` into a directory (`manifest.json` + `blocks/<name>`).
    Unpack { file: PathBuf, outdir: PathBuf },
    /// Pack an exploded directory (`manifest.json` + `blocks/`) into a sealed `.tsra`.
    Pack { dir: PathBuf, out: PathBuf },
    /// Validate a `.tsra`'s manifest against its declared product schema (required fields/blocks).
    Schema { file: PathBuf },
    /// Ingest a vendor acquisition file into a sealed `.tsra` product (normalise at the door).
    Ingest {
        #[command(subcommand)]
        src: IngestSrc,
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
            let r = Reader::open(&file)?;
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
            let mut r = Reader::open(&file)?; // magic + manifest seal
            let n = r.manifest().blocks.len();
            for name in r.block_names() {
                r.read_block(&name)?; // payload bytes vs recorded digest
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
        Cmd::Ingest { src } => run_ingest(src),
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
            if tessera_io::verify_tsra(&file, &vk)? {
                println!("OK  {} signature valid", file.display());
                Ok(())
            } else {
                Err(tessera_core::Error::Invalid(format!(
                    "signature INVALID for {}",
                    file.display()
                )))
            }
        }
    }
}

fn run_ingest(src: IngestSrc) -> tessera_core::Result<()> {
    let (manifest, payloads, out) = match src {
        IngestSrc::Dicom {
            input,
            out,
            name,
            timestamp,
            deidentify,
        } => {
            let img = if deidentify {
                dicom::read_image_deidentified(&input)?
            } else {
                dicom::read_image(&input)?
            };
            let source = input.to_string_lossy();
            let (m, p) = dicom::to_recon_product(&img, &name, &timestamp, &source)?;
            (m, p, out)
        }
        IngestSrc::GeHdf5 {
            input,
            out,
            name,
            timestamp,
            dataset,
        } => {
            let cols = match dataset.as_str() {
                "events_3p" => ge_hdf5::read_events_3p(&input, &dataset)?,
                "events_2p" => ge_hdf5::read_events_2p(&input, &dataset)?,
                other => {
                    return Err(tessera_core::Error::Invalid(format!(
                        "unknown GE dataset '{other}' (expected events_2p or events_3p)"
                    )))
                }
            };
            let source = input.to_string_lossy();
            let (m, p) = ge_hdf5::to_listmode_product(&cols, &name, &timestamp, &source)?;
            (m, p, out)
        }
    };
    pack(&manifest, &payloads, &out)?;
    println!(
        "ingested {} ({} blocks) -> {}",
        manifest.id,
        manifest.blocks.len(),
        out.display()
    );
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
            src: IngestSrc::GeHdf5 {
                input: h5,
                out: out.clone(),
                name: "DP06-lm".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                dataset: "events_3p".into(),
            },
        })
        .unwrap();

        // the ingested product verifies (seal + block digest) and is schema-valid
        run(Cmd::Verify { file: out.clone() }).unwrap();
        run(Cmd::Inspect { file: out.clone() }).unwrap();
        let r = Reader::open(&out).unwrap();
        assert_eq!(r.manifest().product, "listmode");
        assert_eq!(r.manifest().sources.len(), 1);
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
