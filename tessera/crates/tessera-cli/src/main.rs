//! `tessera` — pack / unpack / verify / inspect `.tsra` products (ROADMAP P4, #205).
//!
//! A thin shell over `tessera-core` (format/spine) + `tessera-io` (container). Every command
//! that opens a `.tsra` verifies its magic + manifest seal; `verify` additionally checks every
//! block's stored bytes against its recorded digest.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tessera_io::{pack_dir, unpack, Reader};

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
    }
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
}
