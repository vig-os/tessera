//! `tessera collection inspect | ls | verify` — consumer verbs over a `collection.json` descriptor
//! (ADR-0033 / #272). A collection is the content-addressed catalog the declarative ingest engine
//! writes at `<out_dir>/collection.json`, with its seal beside it in `collection.seal` (ADR-0066
//! rule 5), pinning each member `.tsra` by its `manifest_hash`. These
//! verbs let a downstream consumer read + integrity-check the catalog and its members without
//! re-running the ingest — the collection-level analogue of `tessera inspect` / `verify`.
//!
//! Members are referenced by their content-addressed `id`; on disk a **product** member is
//! `<dir>/<reference>.tsra` and a **sub-collection** member is `<dir>/<reference>.collection.json`
//! with `<dir>/<reference>.collection.seal` (resolved by `kind`, ADR-0049 §4). `ls` opens each member to show its human `name` (the child
//! collection's name for a sub-collection); `verify` **recurses** into sub-collection members,
//! checking each descendant's seal + pinned version transitively (ADR-0049 §3).

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::collection::{
    member_filename, Collection, CollectionBuilder, MemberKind, ProductHandle, Role,
};
use tessera_core::{Error, Result};
use tessera_io::Reader;

/// The collection seal badge (#268), from the full stored-bytes read
/// ([`Collection::from_stored_members`], ADR-0066): `sealed✓` when it verifies, `unsealed` when
/// there is no `collection.seal` (the alpha layout, which `verify` refuses), `sealed✗` for any other
/// refusal (a seal mismatch, non-canonical bytes, a member rule).
fn seal_status(file: &Path, bytes: &[u8], seal: Option<&[u8]>) -> &'static str {
    match tessera_io::stored::collection_from_pair(file, bytes, seal) {
        Ok(_) => "sealed✓",
        Err(Error::UnsupportedLayout { .. }) => "unsealed",
        Err(_) => "sealed✗",
    }
}

/// Warn on stderr that `c` carries fields from a newer format revision (verified, not interpreted).
fn warn_newer_fields(c: &Collection) {
    crate::warn_not_interpreted(
        &format!("collection '{}': fields", c.name),
        c.newer_fields(),
    );
}

fn role_str(r: &Role) -> &'static str {
    match r {
        Role::Raw => "raw",
        Role::Derived => "derived",
    }
}

/// Load a `collection.json` **without** verifying (so `inspect` can honestly render a tampered one).
fn load(file: &Path) -> Result<Collection> {
    Collection::from_json(&String::from_utf8_lossy(&std::fs::read(file)?))
}

/// The on-disk path of a member next to the `collection.json` — resolved by `kind` (ADR-0049 §4) via
/// the shared [`member_filename`] SSoT (so the resolved name matches what every writer emits, #323).
fn member_path(collection_file: &Path, reference: &str, kind: MemberKind) -> PathBuf {
    let dir = collection_file.parent().unwrap_or_else(|| Path::new("."));
    dir.join(member_filename(reference, kind))
}

fn w(out: &mut dyn Write, args: std::fmt::Arguments<'_>) -> Result<()> {
    out.write_fmt(args).map_err(Error::from)
}

/// `tessera collection inspect FILE` — the catalog header: identity, seal badge, and each member's
/// role + reference + pinned `manifest_hash` (short). Payload-cheap — reads only the descriptor.
pub fn inspect(file: &Path, out: &mut dyn Write) -> Result<()> {
    let (bytes, seal) = tessera_io::stored::read_pair(file)?;
    let c = Collection::from_json(&String::from_utf8_lossy(&bytes))?;
    let recorded = seal.as_deref().map(String::from_utf8_lossy);
    // Without a seal file this is the alpha layout (badge `unsealed`), whose in-band
    // `manifest_hash` is not a newer-format field: only a document in the ADR-0066 layout has any.
    if seal.is_some() {
        warn_newer_fields(&c);
    }
    w(out, format_args!("collection {}\n", c.name))?;
    w(out, format_args!("id            {}\n", c.id))?;
    w(out, format_args!("schema        {}\n", c.collection_schema))?;
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
        format_args!("manifest_hash {}\n", recorded.as_deref().unwrap_or("-")),
    )?;
    w(
        out,
        format_args!(
            "seal          {}\n",
            seal_status(file, &bytes, seal.as_deref())
        ),
    )?;
    w(out, format_args!("members       {}\n", c.members.len()))?;
    for m in &c.members {
        let kind = if m.kind == MemberKind::Collection {
            "collection"
        } else {
            "product"
        };
        w(
            out,
            format_args!(
                "  - {:<7} {:<10} {}  [{}]\n",
                role_str(&m.role),
                kind,
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
        let path = member_path(file, &m.reference, m.kind);
        // A product member's name/product comes from its `.tsra`; a sub-collection member's from its
        // child `collection.json` (product shown as `collection`).
        let (product, name) = if m.kind == MemberKind::Collection {
            match load(&path).ok() {
                Some(child) => ("collection".to_string(), child.name),
                None => (
                    "collection".to_string(),
                    "<sub-collection not found>".to_string(),
                ),
            }
        } else {
            match Reader::open(&path) {
                Ok(r) => {
                    let mm = r.manifest();
                    (mm.product.clone(), mm.name.clone())
                }
                Err(_) => ("?".to_string(), "<member file not found>".to_string()),
            }
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

/// `tessera collection verify FILE` — the catalog's end-to-end integrity check, **recursive**
/// (ADR-0049 §3): verify the collection seal (id / content_hash / manifest_hash), then for every
/// member confirm it is present + intact + exactly the version pinned — opening a **product** member's
/// `.tsra` (its own seal) or descending into a **sub-collection** member's `collection.json` and
/// verifying it transitively. A missing, mismatched, or unreachable member is a typed [`Error`]; a
/// cycle (defense-in-depth — content-addressing is acyclic by construction) is refused. Offline: a
/// child that can't be read fails loudly rather than passing silently.
pub fn verify(file: &Path, out: &mut dyn Write) -> Result<()> {
    let mut seen = HashSet::new();
    let n = verify_collection(file, None, &mut seen)?;
    w(
        out,
        format_args!("OK  {} verified ({n} members, recursive)\n", file.display()),
    )
}

/// Recursively verify the collection at `file`. `expected_mh` (when descending from a parent) pins the
/// exact version the parent recorded. Returns the count of members verified in this subtree.
fn verify_collection(
    file: &Path,
    expected_mh: Option<&str>,
    seen: &mut HashSet<String>,
) -> Result<usize> {
    let (bytes, seal) = tessera_io::stored::read_pair(file).map_err(|e| {
        Error::Invalid(format!(
            "sub-collection unreachable at {} — cannot fully verify offline: {e}",
            file.display()
        ))
    })?;
    // Seal check (the stored bytes against `collection.seal`, then id + content_hash over the
    // pinned member hashes and the level schema).
    let c = tessera_io::stored::collection_from_pair(file, &bytes, seal.as_deref())?;
    warn_newer_fields(&c);
    // Confirm this collection is the exact version its parent pinned (swapped-but-valid = failure).
    if let Some(exp) = expected_mh {
        let got = c.manifest_hash.as_deref().unwrap_or_default();
        if got != exp {
            return Err(Error::Integrity {
                what: "collection_member",
                expected: exp.to_string(),
                actual: got.to_string(),
            });
        }
    }
    // Cycle guard (defense-in-depth; a member can't pin an ancestor hash that doesn't exist yet).
    if let Some(mh) = &c.manifest_hash {
        if !seen.insert(mh.clone()) {
            return Err(Error::Invalid(format!(
                "collection cycle detected at {} (manifest_hash already visited)",
                file.display()
            )));
        }
    }

    let mut count = 0usize;
    for m in &c.members {
        let path = member_path(file, &m.reference, m.kind);
        match m.kind {
            MemberKind::Collection => {
                // Descend: the child's own seal + pinned version + its whole subtree.
                count += verify_collection(&path, Some(&m.manifest_hash), seen)?;
            }
            _ => {
                // Product member: `Reader::open` verifies its seal; confirm the pinned version.
                let r = Reader::open(&path).map_err(|e| {
                    Error::Invalid(format!(
                        "collection member '{}' missing or unreadable at {}: {e}",
                        m.reference,
                        path.display()
                    ))
                })?;
                let got = r.manifest().manifest_hash.as_deref().unwrap_or_default();
                if got != m.manifest_hash {
                    return Err(Error::Integrity {
                        what: "collection_member",
                        expected: m.manifest_hash.clone(),
                        actual: got.to_string(),
                    });
                }
            }
        }
        count += 1;
    }
    Ok(count)
}

/// `tessera collection new` — assemble a `collection.json` from **pre-sealed** `.tsra` products
/// (#304 Part 1), decoupling "which members belong together" from ingest time. Opens each member
/// (which verifies its seal), pins it by `(id, manifest_hash)` via the typed [`ProductHandle`], seals
/// the catalog under the declared `--schema` (validating its `member_rule`), and writes a
/// self-contained collection directory: `<out>/collection.json` + `<out>/collection.seal` + each
/// member hard-linked (or copied)
/// as `<out>/<id>.tsra`, so `collection verify` resolves + checks it in place. The collection's
/// timestamp defaults to the latest member's (deterministic) unless `--timestamp` is given.
#[allow(clippy::too_many_arguments)]
pub fn new(
    members: &[PathBuf],
    out: &Path,
    name: &str,
    description: &str,
    schema: &str,
    study: Option<&str>,
    timestamp: Option<&str>,
    role: Role,
    w_out: &mut dyn Write,
) -> Result<()> {
    if members.is_empty() {
        return Err(Error::Invalid(
            "collection new needs at least one --member".into(),
        ));
    }
    // Open each member (verifies its own seal), pin by (id, manifest_hash), track the latest ts.
    let mut handles: Vec<(ProductHandle, PathBuf)> = Vec::with_capacity(members.len());
    let mut latest_ts = String::new();
    for m in members {
        let r = Reader::open(m).map_err(|e| {
            Error::Invalid(format!(
                "member '{}' is not a readable .tsra: {e}",
                m.display()
            ))
        })?;
        let mf = r.manifest();
        if mf.timestamp > latest_ts {
            latest_ts = mf.timestamp.clone();
        }
        handles.push((ProductHandle::of(mf)?, m.clone()));
    }
    let ts = timestamp.map(String::from).unwrap_or(latest_ts);

    let mut cb = CollectionBuilder::new(name, description, &ts);
    cb.with_collection_schema(schema);
    if let Some(s) = study {
        cb.with_study(s);
    }
    for (h, _) in &handles {
        cb.add_product(h, role, Vec::new());
    }
    // Seal validates the declared schema's `member_rule` (a `dataset` accepts these products; a
    // `project` would reject them — fail-fast before writing anything).
    let sealed = cb.seal()?;

    // Write the self-contained collection dir: collection.json + collection.seal + each member
    // under the shared `member_filename` (sanitized `<stem>.tsra`), the same name `verify`/`ls`
    // resolve (#323).
    std::fs::create_dir_all(out)?;
    tessera_io::stored::write_collection(&out.join("collection.json"), &sealed)?;
    for (h, src) in &handles {
        let dst = out.join(member_filename(h.reference(), MemberKind::Product));
        if src.canonicalize().ok() == dst.canonicalize().ok() {
            continue; // member already in place
        }
        let _ = std::fs::remove_file(&dst);
        // Hard-link when same-filesystem (no data copy); fall back to a full copy across mounts.
        if std::fs::hard_link(src, &dst).is_err() {
            std::fs::copy(src, &dst)?;
        }
    }
    w(
        w_out,
        format_args!(
            "wrote collection '{}' (schema {}, {} members) → {}\n",
            sealed.id,
            sealed.collection_schema,
            sealed.members.len(),
            out.join("collection.json").display()
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
                &dir.join(member_filename(&sealed.id, MemberKind::Product)),
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
        tessera_io::stored::write_collection(&dir.join("collection.json"), &c).unwrap();
        (c, ids)
    }

    /// A collection a newer writer sealed with a field this build doesn't model: `inspect` shows the
    /// seal as holding (it is computed over the object as parsed, #873) and `verify` passes; the same
    /// field spliced in under the old seal shows `sealed✗` and fails `verify`.
    #[test]
    fn a_newer_collection_field_verifies_and_a_spliced_one_does_not() {
        let dir = tempfile::tempdir().unwrap();
        build_collection(dir.path());
        let cf = dir.path().join("collection.json");
        let sf = dir.path().join("collection.seal");
        let old_seal = std::fs::read_to_string(&sf).unwrap();
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&cf).unwrap()).unwrap();
        v["annotations"] = serde_json::json!({ "curator": "site-a" });
        let bytes = tessera_core::canonical::to_bytes(&v).unwrap();
        let new_seal = tessera_core::hash::digest(&bytes);
        let inspect_seal = |seal: &str| {
            std::fs::write(&cf, &bytes).unwrap();
            std::fs::write(&sf, seal).unwrap();
            let mut buf = Vec::new();
            inspect(&cf, &mut buf).unwrap();
            let s = String::from_utf8(buf).unwrap();
            s.lines()
                .find(|l| l.starts_with("seal"))
                .unwrap()
                .to_string()
        };
        assert_eq!(inspect_seal(&new_seal), "seal          sealed✓");
        verify(&cf, &mut Vec::new()).expect("a sealed newer field verifies");
        assert_eq!(inspect_seal(&old_seal), "seal          sealed✗");
        assert!(matches!(
            verify(&cf, &mut Vec::new()),
            Err(Error::Integrity {
                what: "manifest_hash",
                ..
            })
        ));
    }

    /// A crafted top-level key spelling a member field's path (`"members[0].kind"`) is a newer field
    /// of its own, never an exemption for the member's `kind`: that `kind` spelled as an object is
    /// refused under a fresh seal, where `verify` once passed it with a warning (#873).
    #[test]
    fn a_crafted_key_cannot_exempt_a_member_field() {
        let dir = tempfile::tempdir().unwrap();
        build_collection(dir.path());
        let cf = dir.path().join("collection.json");
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&cf).unwrap()).unwrap();
        v["members[0].kind"] = serde_json::json!(1);
        v["members"][0]["kind"] = serde_json::json!({ "product": null });
        let bytes = tessera_core::canonical::to_bytes(&v).unwrap();
        tessera_io::stored::write_pair(&cf, &bytes, &tessera_core::hash::digest(&bytes)).unwrap();
        let err = verify(&cf, &mut Vec::new()).expect_err("a second spelling must not verify");
        assert!(
            err.to_string().contains("`members[0].kind` is an object"),
            "{err}"
        );
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
            .contains("verified (2 members, recursive)"));

        // Remove a member file → verify fails loudly (missing member).
        std::fs::remove_file(dir.path().join(member_filename(
            &c.members[0].reference,
            MemberKind::Product,
        )))
        .unwrap();
        assert!(verify(&cf, &mut Vec::new()).is_err());
    }

    /// Build a child collection on disk written as `<id>.collection.json` (+ its member `.tsra`s).
    fn build_child_collection(dir: &Path, name: &str) -> Collection {
        let mut cb = CollectionBuilder::new(name, "a child dataset", "2024-01-01T00:00:00Z");
        for pname in ["ct", "pt"] {
            let spec = ArraySpec::new(vec![2, 2], "int16");
            let (bref, payload) =
                tessera_io::array::array_block("volume", &spec, &ArrayData::I16(vec![0, 1, 2, 3]))
                    .unwrap();
            let mut b = ProductBuilder::new("recon", pname, "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(bref);
            let sealed = b.seal().unwrap();
            pack(
                &sealed,
                &[payload],
                &dir.join(member_filename(&sealed.id, MemberKind::Product)),
            )
            .unwrap();
            cb.add_member(
                &sealed.id,
                sealed.manifest_hash.clone().unwrap(),
                Role::Raw,
                Vec::new(),
            );
        }
        let c = cb.seal().unwrap();
        tessera_io::stored::write_collection(
            &dir.join(member_filename(&c.id, MemberKind::Collection)),
            &c,
        )
        .unwrap();
        c
    }

    /// Part 2 (ADR-0049 §3): `verify` recurses into a sub-collection member (a `project` over a
    /// `dataset`), `inspect` shows its kind, `ls` resolves its name, and a tampered *grand*child is
    /// caught transitively.
    #[test]
    fn verify_recurses_into_a_sub_collection() {
        use tessera_core::collection::CollectionHandle;
        let dir = tempfile::tempdir().unwrap();
        let child = build_child_collection(dir.path(), "dataset-A");

        // Parent "project" with the child dataset as a sub-collection member.
        let mut cb = CollectionBuilder::new("project-X", "a project", "2024-01-01T00:00:00Z");
        cb.add_subcollection(
            &CollectionHandle::of(&child).unwrap(),
            Role::Derived,
            Vec::new(),
        );
        let parent = cb.seal().unwrap();
        let pf = dir.path().join("collection.json");
        tessera_io::stored::write_collection(&pf, &parent).unwrap();

        // inspect: the member line shows `collection` kind for the sub-collection.
        let mut buf = Vec::new();
        inspect(&pf, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.lines()
                .any(|l| l.contains(&child.id) && l.contains("collection")),
            "{s}"
        );

        // ls: the sub-collection member resolves to the child's human name.
        let mut buf = Vec::new();
        ls(&pf, false, &mut buf).unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("dataset-A"));

        // verify: recurses → the sub-collection (1) + its two products = 3 members.
        let mut buf = Vec::new();
        verify(&pf, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("recursive") && s.contains("3 members"), "{s}");

        // A sub-collection without its seal file is the alpha layout, refused transitively.
        let child_seal = dir.path().join(format!(
            "{}.collection.seal",
            tessera_core::collection::sanitize_reference(&child.id)
        ));
        let kept = std::fs::read(&child_seal).unwrap();
        std::fs::remove_file(&child_seal).unwrap();
        assert!(matches!(
            verify(&pf, &mut Vec::new()),
            Err(Error::UnsupportedLayout { .. })
        ));
        std::fs::write(&child_seal, kept).unwrap();
        verify(&pf, &mut Vec::new()).unwrap();

        // Tamper a grandchild product `.tsra` → the recursive verify catches it transitively.
        std::fs::remove_file(dir.path().join(member_filename(
            &child.members[0].reference,
            MemberKind::Product,
        )))
        .unwrap();
        assert!(verify(&pf, &mut Vec::new()).is_err());
    }

    /// `collection new` (#304 Part 1): assemble a `dataset` from two pre-sealed `.tsra` products →
    /// a self-contained dir (`collection.json` + `<id>.tsra` members) that `verify` passes, and
    /// `inspect` shows the declared schema.
    #[test]
    fn collection_new_assembles_from_presealed_tsra() {
        let dir = tempfile::tempdir().unwrap();
        let mut member_paths = Vec::new();
        for name in ["ct", "pt"] {
            let spec = ArraySpec::new(vec![2, 2], "int16");
            let (bref, payload) =
                tessera_io::array::array_block("volume", &spec, &ArrayData::I16(vec![0, 1, 2, 3]))
                    .unwrap();
            let mut b = ProductBuilder::new("recon", name, "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(bref);
            let sealed = b.seal().unwrap();
            let p = dir.path().join(format!("input_{name}.tsra"));
            pack(&sealed, &[payload], &p).unwrap();
            member_paths.push(p);
        }
        let out = dir.path().join("assembled");
        new(
            &member_paths,
            &out,
            "study-A",
            "",
            "dataset",
            Some("S1"),
            None,
            Role::Raw,
            &mut Vec::new(),
        )
        .unwrap();

        let cf = out.join("collection.json");
        assert!(cf.exists(), "collection.json written");
        // The collection writer stores `collection.json` in its JCS form with its seal beside it.
        crate::test_support::assert_stored_pair(
            "collection new",
            &std::fs::read(&cf).unwrap(),
            &std::fs::read(out.join("collection.seal")).unwrap(),
        );
        // #323: each member lands under the sanitized stem (`blake3_….tsra`), NOT the raw
        // colon-bearing reference — and that is exactly the name `verify` resolves.
        let c = load(&cf).unwrap();
        for m in &c.members {
            assert!(m.reference.contains(':'), "a real blake3 id has a colon");
            assert!(
                out.join(member_filename(&m.reference, MemberKind::Product))
                    .exists(),
                "member resolves under the sanitized stem"
            );
            assert!(
                !out.join(format!("{}.tsra", m.reference)).exists(),
                "the raw colon-named file must NOT be the on-disk name (the #323 mismatch)"
            );
        }
        // The assembled collection is self-contained + passes recursive verify.
        verify(&cf, &mut Vec::new()).unwrap();
        let mut buf = Vec::new();
        inspect(&cf, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("schema        dataset") && s.contains("members       2"),
            "{s}"
        );

        // A `project` schema rejects product members (member_rule) — fail-fast, nothing written.
        let out2 = dir.path().join("bad");
        assert!(new(
            &member_paths,
            &out2,
            "p",
            "",
            "project",
            None,
            None,
            Role::Derived,
            &mut Vec::new()
        )
        .is_err());
    }
}
