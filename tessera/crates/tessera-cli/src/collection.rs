//! `tessera collection inspect | ls | verify` — consumer verbs over a `collection.json` descriptor
//! (ADR-0033 / #272). A collection is the content-addressed catalog the declarative ingest engine
//! writes at `<out_dir>/collection.json`, pinning each member `.tsra` by its `manifest_hash`. These
//! verbs let a downstream consumer read + integrity-check the catalog and its members without
//! re-running the ingest — the collection-level analogue of `tessera inspect` / `verify`.
//!
//! Members are referenced by their content-addressed `id`; on disk each is `<dir>/<reference>.tsra`
//! next to the `collection.json` (the `prefix_layout` / engine convention). `ls` opens each member
//! to show its human `name` + product (the "human-friendly" view over the hash-named files).

use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::collection::{Collection, Role};
use tessera_core::{Error, Result};
use tessera_io::Reader;

/// The collection seal badge (mirrors `nav::seal_status` for products, #268): `sealed✓` when
/// `manifest_hash` is present AND re-verifies over the canonical bytes, `sealed✗` on a mismatch,
/// `unsealed` when the collection carries no seal.
fn seal_status(c: &Collection) -> &'static str {
    match &c.manifest_hash {
        None => "unsealed",
        Some(mh) => match c.compute_manifest_hash() {
            Ok(got) if &got == mh => "sealed✓",
            _ => "sealed✗",
        },
    }
}

fn role_str(r: &Role) -> &'static str {
    match r {
        Role::Raw => "raw",
        Role::Derived => "derived",
    }
}

/// Load a `collection.json` **without** verifying (so `inspect` can honestly render a tampered one).
fn load(file: &Path) -> Result<Collection> {
    Collection::from_json(&std::fs::read_to_string(file)?)
}

/// The on-disk path of a member `.tsra`: `<reference>.tsra` next to the `collection.json`.
fn member_path(collection_file: &Path, reference: &str) -> PathBuf {
    collection_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{reference}.tsra"))
}

fn w(out: &mut dyn Write, args: std::fmt::Arguments<'_>) -> Result<()> {
    out.write_fmt(args).map_err(Error::from)
}

/// `tessera collection inspect FILE` — the catalog header: identity, seal badge, and each member's
/// role + reference + pinned `manifest_hash` (short). Payload-cheap — reads only the descriptor.
pub fn inspect(file: &Path, out: &mut dyn Write) -> Result<()> {
    let c = load(file)?;
    w(out, format_args!("collection {}\n", c.name))?;
    w(out, format_args!("id            {}\n", c.id))?;
    w(out, format_args!("timestamp     {}\n", c.timestamp))?;
    if let Some(s) = &c.study {
        w(out, format_args!("study         {s}\n"))?;
    }
    w(
        out,
        format_args!(
            "content_hash  {}\n",
            c.content_hash.as_deref().unwrap_or("-")
        ),
    )?;
    w(
        out,
        format_args!(
            "manifest_hash {}\n",
            c.manifest_hash.as_deref().unwrap_or("-")
        ),
    )?;
    w(out, format_args!("seal          {}\n", seal_status(&c)))?;
    w(out, format_args!("members       {}\n", c.members.len()))?;
    for m in &c.members {
        w(
            out,
            format_args!(
                "  - {:<7} {}  [{}]\n",
                role_str(&m.role),
                m.reference,
                crate::nav::short_hash(&m.manifest_hash)
            ),
        )?;
    }
    Ok(())
}

/// `tessera collection ls FILE` — one line per member with its **human** name + product, resolved by
/// opening each member `.tsra` (falls back to `?` if a member file is absent). `--full` also prints
/// the full pinned `manifest_hash` and any in-collection `derived_from` edges.
pub fn ls(file: &Path, full: bool, out: &mut dyn Write) -> Result<()> {
    let c = load(file)?;
    for m in &c.members {
        let path = member_path(file, &m.reference);
        let (product, name) = match Reader::open(&path) {
            Ok(r) => {
                let mm = r.manifest();
                (mm.product.clone(), mm.name.clone())
            }
            Err(_) => ("?".to_string(), "<member file not found>".to_string()),
        };
        let hash = if full {
            m.manifest_hash.clone()
        } else {
            crate::nav::short_hash(&m.manifest_hash)
        };
        w(
            out,
            format_args!(
                "{:<7} {:<10} {:<20} {name:<24} [{hash}]\n",
                role_str(&m.role),
                product,
                m.reference,
            ),
        )?;
        if full && !m.derived_from.is_empty() {
            w(
                out,
                format_args!("          derived_from: {}\n", m.derived_from.join(", ")),
            )?;
        }
    }
    Ok(())
}

/// `tessera collection verify FILE` — verify the collection seal (id / content_hash / manifest_hash),
/// then open every member `.tsra` and check its own seal AND that its `manifest_hash` equals the one
/// the collection pinned. A missing or mismatched member is a typed [`Error`]. This is the catalog's
/// end-to-end integrity check: the collection commits to its members, and every member is present +
/// intact + exactly the version the catalog recorded.
pub fn verify(file: &Path, out: &mut dyn Write) -> Result<()> {
    // Collection seal first (id + content_hash over pinned member hashes + manifest_hash).
    let c = Collection::from_json_verified(&std::fs::read_to_string(file)?)?;
    for m in &c.members {
        let path = member_path(file, &m.reference);
        let r = Reader::open(&path).map_err(|e| {
            Error::Invalid(format!(
                "collection member '{}' missing or unreadable at {}: {e}",
                m.reference,
                path.display()
            ))
        })?;
        // The member's own seal was verified by `Reader::open`; now confirm it is the exact version
        // the collection pinned (a swapped-but-valid member is still a collection-integrity failure).
        let got = r.manifest().manifest_hash.as_deref().unwrap_or_default();
        if got != m.manifest_hash {
            return Err(Error::Integrity {
                what: "collection_member",
                expected: m.manifest_hash.clone(),
                actual: got.to_string(),
            });
        }
    }
    w(
        out,
        format_args!(
            "OK  {} verified ({} members)\n",
            file.display(),
            c.members.len()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::collection::CollectionBuilder;
    use tessera_core::ProductBuilder;
    use tessera_io::{array::ArrayData, pack};

    /// Build a 2-member collection on disk (`collection.json` + two `<id>.tsra`) and return its dir.
    fn build_collection(dir: &Path) -> (Collection, Vec<String>) {
        let mut ids = Vec::new();
        let mut cb = CollectionBuilder::new("study", "a CT+PET study", "2024-01-01T00:00:00Z");
        for name in ["ct", "pt"] {
            let spec = ArraySpec::new(vec![2, 2], "int16");
            let (bref, payload) =
                tessera_io::array::array_block("volume", &spec, &ArrayData::I16(vec![0, 1, 2, 3]))
                    .unwrap();
            let mut b = ProductBuilder::new("recon", name, "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(bref);
            let sealed = b.seal().unwrap();
            pack(
                &sealed,
                &[payload],
                &dir.join(format!("{}.tsra", sealed.id)),
            )
            .unwrap();
            cb.add_member(
                &sealed.id,
                sealed.manifest_hash.clone().unwrap(),
                Role::Raw,
                Vec::new(),
            );
            ids.push(sealed.id.clone());
        }
        let c = cb.seal().unwrap();
        std::fs::write(dir.join("collection.json"), c.to_json().unwrap()).unwrap();
        (c, ids)
    }

    #[test]
    fn inspect_ls_verify_a_collection_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _ids) = build_collection(dir.path());
        let cf = dir.path().join("collection.json");

        let mut buf = Vec::new();
        inspect(&cf, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("collection study"));
        assert!(s.contains("seal          sealed✓"), "{s}");
        assert!(s.contains("members       2"));

        let mut buf = Vec::new();
        ls(&cf, false, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // The human name + product of each member (resolved by opening the .tsra) show up.
        assert!(
            s.contains("recon") && s.contains("ct") && s.contains("pt"),
            "{s}"
        );

        // Full verify: collection seal + every member present, intact, and the pinned version.
        let mut buf = Vec::new();
        verify(&cf, &mut buf).unwrap();
        assert!(String::from_utf8(buf)
            .unwrap()
            .contains("verified (2 members)"));

        // Remove a member file → verify fails loudly (missing member).
        std::fs::remove_file(dir.path().join(format!("{}.tsra", c.members[0].reference))).unwrap();
        assert!(verify(&cf, &mut Vec::new()).is_err());
    }
}
