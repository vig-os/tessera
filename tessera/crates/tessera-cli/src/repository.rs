//! Repository collection and pin commands (ADR-0067 T8).
//!
//! The caller supplies sealed collections, including version edges and fork identity inputs.
//! No command invents a token or reads a tip to substitute for a caller's CAS expectation.
use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::{Error, ObjectKind, Result};
use tessera_io::repo::PinTarget;
use tessera_io::{BlockPayload, NewMember, Reader, Repository};

/// Target kind exposed in pin help, checked by the repository when setting the pin.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Kind {
    /// A sealed product manifest.
    Product,
    /// A sealed collection.
    Collection,
}

impl From<Kind> for ObjectKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Product => Self::Product,
            Kind::Collection => Self::Collection,
        }
    }
}

fn kind_name(kind: ObjectKind) -> String {
    format!("{kind:?}").to_lowercase()
}

/// Commit the supplied sealed catalog under its stable id, with all new members atomically.
/// Product payloads are encoded bytes read by block name; no encoding or GC window is introduced.
/// Sub-collection closure is checked by the repository, including members supplied in any order.
pub fn commit(
    path: &Path,
    file: &Path,
    expected: Option<&str>,
    members: &[PathBuf],
    collections: &[PathBuf],
    out: &mut dyn Write,
    target: &mut Option<crate::exit::Target>,
) -> Result<()> {
    let repo = Repository::open(path)?;
    let collection = tessera_io::stored::read_collection(file)?;
    *target = Some(crate::exit::Target::Lineage(collection.id.clone()));
    let mut products = Vec::new();
    for file in members {
        let mut reader = Reader::open(file)?;
        let manifest = reader.manifest().clone();
        let mut payloads = Vec::new();
        for block in &manifest.blocks {
            payloads.push(BlockPayload::new(
                block.name.clone(),
                reader.read_block(&block.name)?,
            ));
        }
        products.push((manifest, payloads));
    }
    let collections = collections
        .iter()
        .map(|p| tessera_io::stored::read_collection(p))
        .collect::<Result<Vec<_>>>()?;
    let mut supplied = products
        .iter()
        .map(|(manifest, payloads)| NewMember::Product { manifest, payloads })
        .collect::<Vec<_>>();
    supplied.extend(collections.iter().map(NewMember::Collection));
    let seal = repo.commit_collection(&collection.id, expected, &collection, &supplied)?;
    writeln!(
        out,
        "committed collection {}\n  version {seal}",
        collection.id
    )?;
    Ok(())
}

/// Print the oldest-first lineage cache, or the complete DAG in the library's history order.
/// History is fully validated before printing, so a missing predecessor cannot look complete.
/// Plain log reads the tip and cache separately without a lock. A concurrent commit may add entries
/// beyond the observed tip; forget/GC may cause a read error. The tip marker is the initial observation.
pub fn log(path: &Path, reference: &str, history: bool, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(path)?;
    if history {
        // Enforce the collection command's target kind before walking either kind of predecessor.
        repo.get_collection(reference)?;
        for entry in repo.history(reference)? {
            writeln!(
                out,
                "{} {} {}",
                entry.address,
                kind_name(entry.kind),
                entry.id
            )?;
            for edge in entry.predecessors {
                writeln!(out, "  {} {}", edge.role, edge.target)?;
            }
        }
    } else {
        let tip = repo
            .read_ref(reference)?
            .ok_or_else(|| Error::Invalid(format!("no collection lineage '{reference}'")))?;
        repo.get_collection(&tip)?;
        for entry in repo.log(reference)? {
            let tag = if entry.manifest_hash == tip {
                "  (tip)"
            } else {
                ""
            };
            writeln!(out, "{}{tag}", entry.manifest_hash)?;
        }
    }
    Ok(())
}

/// Print the pure immediate-member diff in its specified old/new member order.
/// Metadata, roles, derivation edges and nested member contents are outside this diff.
pub fn diff(path: &Path, old: &str, new: &str, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(path)?;
    let delta =
        tessera_core::collection::diff(&repo.get_collection(old)?, &repo.get_collection(new)?)?;
    for member in delta.added {
        writeln!(
            out,
            "+ {} {} {}",
            member.reference,
            format!("{:?}", member.kind).to_lowercase(),
            member.manifest_hash
        )?;
    }
    for member in delta.removed {
        writeln!(out, "- {} {}", member.reference, member.manifest_hash)?;
    }
    for member in delta.repinned {
        writeln!(
            out,
            "~ {} {} -> {}",
            member.reference, member.old_manifest_hash, member.new_manifest_hash
        )?;
    }
    writeln!(out, "order_changed: {}", delta.order_changed)?;
    Ok(())
}

/// Set a typed pin with the caller's explicit CAS expectation; omission means absent.
pub fn set_pin(
    path: &Path,
    name: &str,
    address: &str,
    kind: Kind,
    expected: Option<&str>,
    out: &mut dyn Write,
) -> Result<()> {
    Repository::open(path)?.set_pin(
        name,
        expected,
        &PinTarget {
            kind: kind.into(),
            address: address.into(),
        },
    )?;
    writeln!(
        out,
        "pinned {} {} {address}",
        crate::exit::quote(name),
        kind_name(kind.into())
    )?;
    Ok(())
}

/// Remove only the observed pin. An omitted expectation can only remove an absent pin.
pub fn remove_pin(
    path: &Path,
    name: &str,
    expected: Option<&str>,
    out: &mut dyn Write,
) -> Result<()> {
    let removed = Repository::open(path)?.remove_pin(name, expected)?;
    writeln!(
        out,
        "{} pin {}",
        if removed { "removed" } else { "no" },
        crate::exit::quote(name)
    )?;
    Ok(())
}

/// List verified pins in sorted logical-name order: JSON-quoted name, kind, then full seal.
pub fn pins(path: &Path, out: &mut dyn Write) -> Result<()> {
    for (name, target) in Repository::open(path)?.pins()? {
        writeln!(
            out,
            "{} {} {}",
            crate::exit::quote(&name),
            kind_name(target.kind),
            target.address
        )?;
    }
    Ok(())
}
